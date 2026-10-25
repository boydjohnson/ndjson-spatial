/*
* Copyright 2019 Boyd Johnson
*
* Licensed under the Apache License, Version 2.0 (the "License");
* you may not use this file except in compliance with the License.
* You may obtain a copy of the License at
*
*     http://www.apache.org/licenses/LICENSE-2.0
*
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific language governing permissions and
* limitations under the License.
*/

//! Geometry functions for the expression language, so `select`, `filter`
//! and `sort -k` in `ndjson-spatial` can compute on geometries:
//!
//! ```sh
//! ndjson-spatial select '*, area(d.geometry) as area, centroid(d.geometry) as c'
//! ndjson-spatial filter 'geom_type(d.geometry) == "Polygon" and area(d.geometry) > 1e6'
//! ndjson-spatial select 'extent(d.geometry)'
//! ```
//!
//! A geometry argument is a GeoJSON geometry object, or a Feature (its
//! geometry is used). Geometries are returned as GeoJSON geometry objects.
//! The second argument of `distance`, `intersects`, `contains` and `within`
//! is a WKT string or a geometry. `area` and `length` take lon/lat, as
//! GeoJSON coordinates are, and return square meters and meters on the
//! WGS84 ellipsoid (`geodesic_area` and `geodesic_length` are aliases);
//! `planar_area` and `planar_length` are in the units of the coordinates,
//! for projected data. Lengths of polygons are their perimeters.

use gdal::{
    spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef},
    vector::Geometry as GdalGeometry,
};
use geo::{
    dimensions::Dimensions, Area, BoundingRect, Centroid, CoordsIter, Distance, Euclidean,
    Geodesic, GeodesicArea, HasDimensions, Length, Orient, Point, Rect,
};
use geos::Geom;
use ndjson_common::common::to_geo_json;
use rmath_ast::{Accumulator, Arity, EvalError, Function, FunctionRegistry};
use serde_json::{json, Value};
use std::{any::Any, cell::RefCell, collections::HashMap, convert::TryFrom};

/// A registry with the built-in functions and the geometry functions.
pub fn registry() -> FunctionRegistry {
    let mut registry = FunctionRegistry::default();
    register(&mut registry);
    registry
}

type GeoFn = fn(&'static str, &geo::Geometry<f64>) -> Result<Value, EvalError>;

/// One geometry in, one value out.
const UNARY: &[(&str, GeoFn)] = &[
    ("area", |f, g| number(f, geodesic_area(g))),
    ("geodesic_area", |f, g| number(f, geodesic_area(g))),
    ("planar_area", |f, g| number(f, g.unsigned_area())),
    ("length", |f, g| number(f, length(g, &Geodesic))),
    ("geodesic_length", |f, g| number(f, length(g, &Geodesic))),
    ("planar_length", |f, g| number(f, length(g, &Euclidean))),
    ("centroid", |_, g| {
        Ok(g.centroid().map_or(Value::Null, point))
    }),
    ("bbox", |_, g| {
        Ok(g.bounding_rect().map_or(Value::Null, bbox))
    }),
    ("x", |f, g| coordinate(f, g, |p| p.x())),
    ("y", |f, g| coordinate(f, g, |p| p.y())),
    ("num_points", |_, g| Ok(json!(g.coords_count()))),
    ("is_empty", |_, g| Ok(Value::Bool(g.is_empty()))),
    ("is_valid", |f, g| {
        let valid = to_geos(f, g)?.is_valid().map_err(|e| invalid(f, e))?;
        Ok(Value::Bool(valid))
    }),
    ("point_on_surface", |f, g| {
        geos_op(f, g, |g| g.point_on_surface())
    }),
    ("convex_hull", |f, g| geos_op(f, g, |g| g.convex_hull())),
    ("make_valid", |f, g| geos_op(f, g, |g| g.make_valid())),
    ("to_wkt", |f, g| {
        let wkt = to_geos(f, g)?.to_wkt().map_err(|e| invalid(f, e))?;
        Ok(Value::String(wkt))
    }),
];

/// A geometry and a WKT string or geometry; `a` is the first argument.
type BinaryFn = fn(&geos::Geometry, &geos::Geometry) -> Result<Value, geos::Error>;

const BINARY: &[(&str, BinaryFn)] = &[
    ("distance", |a, b| Ok(json!(a.distance(b)?))),
    ("intersects", |a, b| Ok(Value::Bool(a.intersects(b)?))),
    ("contains", |a, b| Ok(Value::Bool(a.contains(b)?))),
    ("within", |a, b| Ok(Value::Bool(a.within(b)?))),
];

/// A geometry and a number.
type CellFn = fn(&'static str, Point<f64>, u8) -> Result<Value, EvalError>;

/// Cell ids of the centroid: (name, max precision, fn).
const CELLS: &[(&str, u8, CellFn)] = &[
    ("geohash", 12, |_, p, precision| {
        Ok(Value::String(geohash(p, precision)))
    }),
    ("quadkey", 23, |_, p, zoom| {
        Ok(Value::String(quadkey(p, zoom)))
    }),
    ("h3", 15, |f, p, res| {
        let cell = h3o::LatLng::new(p.y(), p.x())
            .map_err(|e| invalid(f, e))?
            .to_cell(h3o::Resolution::try_from(res).map_err(|e| invalid(f, e))?);
        Ok(Value::String(cell.to_string()))
    }),
];

pub fn register(registry: &mut FunctionRegistry) {
    let one = Arity::exactly(1);
    let two = Arity::exactly(2);

    for &(name, f) in UNARY {
        registry.register(Function::scalar(name, one, move |args| {
            f(name, &geometry(name, &args[0])?)
        }));
    }
    registry.register(Function::scalar("geom_type", one, |args| {
        let g = geometry("geom_type", &args[0])?;
        Ok(Value::String(geom_type(&g).to_string()))
    }));
    registry.register(Function::scalar("from_wkt", one, |args| {
        let wkt = string("from_wkt", &args[0])?;
        let g = geos::Geometry::new_from_wkt(wkt).map_err(|e| invalid("from_wkt", e))?;
        from_geos("from_wkt", g)
    }));
    registry.register(Function::scalar("simplify", two, |args| {
        let tolerance = float("simplify", &args[1])?;
        geos_op("simplify", &geometry("simplify", &args[0])?, |g| {
            g.topology_preserve_simplify(tolerance)
        })
    }));
    registry.register(Function::scalar("buffer", two, |args| {
        let width = float("buffer", &args[1])?;
        geos_op("buffer", &geometry("buffer", &args[0])?, |g| {
            g.buffer(width, 8)
        })
    }));
    for &(name, f) in BINARY {
        registry.register(Function::scalar(name, two, move |args| {
            let a = to_geos(name, &geometry(name, &args[0])?)?;
            with_literal(name, &args[1], |b| f(&a, b).map_err(|e| invalid(name, e)))
        }));
    }
    for &(name, max, f) in CELLS {
        registry.register(Function::scalar(name, two, move |args| {
            let precision = match args[1].as_u64() {
                Some(p) if p <= u64::from(max) => p as u8,
                _ => {
                    return Err(invalid(
                        name,
                        format!("precision must be an integer from 0 to {}", max),
                    ))
                }
            };
            let point = geometry(name, &args[0])?
                .centroid()
                .ok_or_else(|| invalid(name, "empty geometry"))?;
            f(name, point, precision)
        }));
    }
    registry.register(Function::scalar("transform", Arity::exactly(3), |args| {
        let from = string("transform", &args[1])?;
        let to = string("transform", &args[2])?;
        transform(&args[0], from, to)
    }));

    registry.register(Function::aggregate("extent", one, || {
        Box::new(Extent::default())
    }));
    registry.register(Function::aggregate("centroid_agg", one, || {
        Box::new(CentroidAgg::default())
    }));
    registry.register(
        Function::aggregate("union_agg", one, || Box::new(UnionAgg::default())).unbounded(),
    );
}

fn invalid(func: &'static str, message: impl ToString) -> EvalError {
    EvalError::Invalid {
        func,
        message: message.to_string(),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn wrong_type(func: &'static str, v: &Value) -> EvalError {
    EvalError::Type {
        op: func,
        left: type_name(v),
        right: None,
    }
}

fn string<'a>(func: &'static str, v: &'a Value) -> Result<&'a str, EvalError> {
    v.as_str().ok_or_else(|| wrong_type(func, v))
}

fn float(func: &'static str, v: &Value) -> Result<f64, EvalError> {
    v.as_f64().ok_or_else(|| wrong_type(func, v))
}

fn number(func: &'static str, f: f64) -> Result<Value, EvalError> {
    serde_json::Number::from_f64(f)
        .map(Value::Number)
        .ok_or(EvalError::NonFinite { op: func })
}

/// A GeoJSON geometry object, or a Feature's geometry.
pub fn geometry(func: &'static str, v: &Value) -> Result<geo::Geometry<f64>, EvalError> {
    let v = match v {
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("Feature") => {
            match o.get("geometry") {
                Some(g) if !g.is_null() => g,
                _ => return Err(invalid(func, "feature has no geometry")),
            }
        }
        Value::Object(_) => v,
        other => return Err(wrong_type(func, other)),
    };
    let g = serde_json::from_value::<geojson::Geometry>(v.clone()).map_err(|e| invalid(func, e))?;
    geo::Geometry::try_from(g).map_err(|e| invalid(func, e))
}

fn to_json(g: &geo::Geometry<f64>) -> Value {
    serde_json::to_value(geojson::Geometry::new(to_geo_json(g))).expect("geometry serializes")
}

fn point(p: Point<f64>) -> Value {
    to_json(&geo::Geometry::Point(p))
}

fn bbox(r: Rect<f64>) -> Value {
    json!([r.min().x, r.min().y, r.max().x, r.max().y])
}

fn geom_type(g: &geo::Geometry<f64>) -> &'static str {
    match g {
        geo::Geometry::Point(_) => "Point",
        geo::Geometry::Line(_) | geo::Geometry::LineString(_) => "LineString",
        geo::Geometry::Polygon(_) | geo::Geometry::Rect(_) | geo::Geometry::Triangle(_) => {
            "Polygon"
        }
        geo::Geometry::MultiPoint(_) => "MultiPoint",
        geo::Geometry::MultiLineString(_) => "MultiLineString",
        geo::Geometry::MultiPolygon(_) => "MultiPolygon",
        geo::Geometry::GeometryCollection(_) => "GeometryCollection",
    }
}

fn coordinate(
    func: &'static str,
    g: &geo::Geometry<f64>,
    f: fn(Point<f64>) -> f64,
) -> Result<Value, EvalError> {
    match g {
        geo::Geometry::Point(p) => number(func, f(*p)),
        other => Err(invalid(
            func,
            format!("expected a Point, got a {}", geom_type(other)),
        )),
    }
}

/// Total length of the lines, with polygons measured along their rings.
/// The area of `g` in square meters, taking its coordinates as lon/lat.
/// Polygons are oriented first: the geodesic area of a clockwise ring is
/// the rest of the Earth, and ESRI and others write exteriors clockwise.
pub fn geodesic_area(g: &geo::Geometry<f64>) -> f64 {
    use geo::{orient::Direction, Geometry::*};
    match g {
        Polygon(p) => p.orient(Direction::Default).geodesic_area_unsigned(),
        MultiPolygon(p) => p.orient(Direction::Default).geodesic_area_unsigned(),
        Rect(r) => geodesic_area(&Polygon(r.to_polygon())),
        Triangle(t) => geodesic_area(&Polygon(t.to_polygon())),
        GeometryCollection(c) => c.iter().map(geodesic_area).sum(),
        Point(_) | MultiPoint(_) | Line(_) | LineString(_) | MultiLineString(_) => 0.0,
    }
}

fn length(g: &geo::Geometry<f64>, metric: &impl Distance<f64, Point<f64>, Point<f64>>) -> f64 {
    use geo::Geometry::*;
    match g {
        Point(_) | MultiPoint(_) => 0.0,
        Line(l) => metric.length(l),
        LineString(l) => metric.length(l),
        MultiLineString(l) => metric.length(l),
        Polygon(p) => std::iter::once(p.exterior())
            .chain(p.interiors())
            .map(|ring| metric.length(ring))
            .sum(),
        MultiPolygon(m) => m.iter().map(|p| length(&Polygon(p.clone()), metric)).sum(),
        Rect(r) => length(&Polygon(r.to_polygon()), metric),
        Triangle(t) => length(&Polygon(t.to_polygon()), metric),
        GeometryCollection(c) => c.iter().map(|g| length(g, metric)).sum(),
    }
}

fn to_geos(func: &'static str, g: &geo::Geometry<f64>) -> Result<geos::Geometry, EvalError> {
    geos::Geometry::try_from(g).map_err(|e| invalid(func, e))
}

fn from_geos(func: &'static str, g: geos::Geometry) -> Result<Value, EvalError> {
    if g.is_empty().map_err(|e| invalid(func, e))? {
        return Ok(Value::Null);
    }
    let g = geo::Geometry::<f64>::try_from(g).map_err(|e| invalid(func, e))?;
    Ok(to_json(&g))
}

fn geos_op(
    func: &'static str,
    g: &geo::Geometry<f64>,
    op: impl FnOnce(&geos::Geometry) -> Result<geos::Geometry, geos::Error>,
) -> Result<Value, EvalError> {
    let result = op(&to_geos(func, g)?).map_err(|e| invalid(func, e))?;
    from_geos(func, result)
}

thread_local! {
    /// The last WKT literal parsed, since it's usually the same on every
    /// record.
    static LITERAL: RefCell<Option<(String, geos::Geometry)>> = const { RefCell::new(None) };
}

/// Call `f` with `v` as a GEOS geometry: a WKT string (parsed once and
/// cached) or a geometry object.
fn with_literal(
    func: &'static str,
    v: &Value,
    f: impl FnOnce(&geos::Geometry) -> Result<Value, EvalError>,
) -> Result<Value, EvalError> {
    match v {
        Value::String(wkt) => LITERAL.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.as_ref().is_none_or(|(cached, _)| cached != wkt) {
                let g = geos::Geometry::new_from_wkt(wkt).map_err(|e| invalid(func, e))?;
                *cache = Some((wkt.clone(), g));
            }
            f(&cache.as_ref().expect("cached above").1)
        }),
        other => f(&to_geos(func, &geometry(func, other)?)?),
    }
}

thread_local! {
    static TRANSFORMS: RefCell<HashMap<(String, String), CoordTransform>> =
        RefCell::new(HashMap::new());
}

fn spatial_ref(definition: &str) -> Result<SpatialRef, EvalError> {
    let mut srs = SpatialRef::from_definition(definition)
        .map_err(|e| invalid("transform", format!("{:?}: {}", definition, e)))?;
    // GeoJSON is always x/y, whatever the authority's axis order.
    srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    Ok(srs)
}

/// Reproject `g` from the CRS `from` to `to`, each anything GDAL accepts
/// (`"EPSG:4326"`, a PROJ string, WKT).
fn transform(g: &Value, from: &str, to: &str) -> Result<Value, EvalError> {
    let func = "transform";
    let json = to_json(&geometry(func, g)?).to_string();
    let mut gdal = GdalGeometry::from_geojson(&json).map_err(|e| invalid(func, e))?;
    TRANSFORMS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let key = (from.to_string(), to.to_string());
        if !cache.contains_key(&key) {
            let t = CoordTransform::new(&spatial_ref(from)?, &spatial_ref(to)?)
                .map_err(|e| invalid(func, e))?;
            cache.insert(key.clone(), t);
        }
        gdal.transform_inplace(&cache[&key])
            .map_err(|e| invalid(func, e))
    })?;
    let out = gdal.json().map_err(|e| invalid(func, e))?;
    serde_json::from_str(&out).map_err(|e| invalid(func, e))
}

const GEOHASH_ALPHABET: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// The geohash of a lon/lat point with `precision` characters.
fn geohash(p: Point<f64>, precision: u8) -> String {
    let (mut lon, mut lat) = ((-180.0, 180.0), (-90.0, 90.0));
    let mut hash = String::with_capacity(precision as usize);
    let mut even = true;
    for _ in 0..precision {
        let mut index = 0;
        for _ in 0..5 {
            let (range, value) = if even {
                (&mut lon, p.x())
            } else {
                (&mut lat, p.y())
            };
            let mid = (range.0 + range.1) / 2.0;
            index <<= 1;
            if value >= mid {
                index |= 1;
                range.0 = mid;
            } else {
                range.1 = mid;
            }
            even = !even;
        }
        hash.push(GEOHASH_ALPHABET[index] as char);
    }
    hash
}

/// The Bing Maps quadkey of the web mercator tile holding a lon/lat point.
fn quadkey(p: Point<f64>, zoom: u8) -> String {
    const MAX_LAT: f64 = 85.051_128_779_806_59;
    let n = (1u64 << zoom) as f64;
    let lat = p.y().clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = ((p.x() + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0) as u64;
    let y = ((1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 * n)
        .floor()
        .clamp(0.0, n - 1.0) as u64;
    (1..=zoom)
        .rev()
        .map(|i| {
            let mask = 1 << (i - 1);
            let digit = u8::from(x & mask != 0) + 2 * u8::from(y & mask != 0);
            (b'0' + digit) as char
        })
        .collect()
}

fn same<T: 'static>(other: &dyn Accumulator) -> &T {
    other
        .as_any()
        .downcast_ref()
        .expect("merging accumulators of different functions")
}

/// `extent(g)`: the bbox of every geometry.
#[derive(Default)]
struct Extent {
    rect: Option<Rect<f64>>,
}

impl Extent {
    fn add(&mut self, r: Rect<f64>) {
        self.rect = Some(match self.rect {
            None => r,
            Some(e) => Rect::new(
                (e.min().x.min(r.min().x), e.min().y.min(r.min().y)),
                (e.max().x.max(r.max().x), e.max().y.max(r.max().y)),
            ),
        });
    }
}

impl Accumulator for Extent {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if args[0].is_null() {
            return Ok(());
        }
        if let Some(r) = geometry("extent", &args[0])?.bounding_rect() {
            self.add(r);
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        if let Some(r) = same::<Extent>(other).rect {
            self.add(r);
        }
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(self.rect.map_or(Value::Null, bbox))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `centroid_agg(g)`: the centroid of all the geometries together. Like a
/// GeometryCollection's centroid, only the highest-dimension geometries
/// count, weighted by area, length or number of points.
#[derive(Default)]
struct CentroidAgg {
    /// `None` until a non-empty geometry is seen.
    dimension: Option<u8>,
    x: f64,
    y: f64,
    weight: f64,
}

impl CentroidAgg {
    fn add(&mut self, dimension: u8, x: f64, y: f64, weight: f64) {
        match self.dimension {
            Some(d) if d > dimension => return,
            Some(d) if d == dimension => {}
            _ => *self = CentroidAgg::default(),
        }
        self.dimension = Some(dimension);
        self.x += x * weight;
        self.y += y * weight;
        self.weight += weight;
    }
}

impl Accumulator for CentroidAgg {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if args[0].is_null() {
            return Ok(());
        }
        let g = geometry("centroid_agg", &args[0])?;
        let centroid = match g.centroid() {
            Some(c) => c,
            None => return Ok(()),
        };
        let (dimension, weight) = match g.dimensions() {
            Dimensions::Empty => return Ok(()),
            Dimensions::ZeroDimensional => (0, g.coords_count() as f64),
            Dimensions::OneDimensional => (1, length(&g, &Euclidean)),
            Dimensions::TwoDimensional => (2, g.unsigned_area()),
        };
        if weight > 0.0 {
            self.add(dimension, centroid.x(), centroid.y(), weight);
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        let other = same::<CentroidAgg>(other);
        if let Some(d) = other.dimension {
            if other.weight > 0.0 {
                self.add(
                    d,
                    other.x / other.weight,
                    other.y / other.weight,
                    other.weight,
                );
            }
        }
    }

    fn finish(&self) -> Result<Value, EvalError> {
        Ok(if self.weight > 0.0 {
            point(Point::new(self.x / self.weight, self.y / self.weight))
        } else {
            Value::Null
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `union_agg(g)`: the union of every geometry. Pending geometries are
/// unioned in batches, so memory follows the size of the union.
#[derive(Default)]
struct UnionAgg {
    union: Option<geo::Geometry<f64>>,
    pending: Vec<geo::Geometry<f64>>,
}

const UNION_BATCH: usize = 256;

impl UnionAgg {
    fn collapse(&mut self) -> Result<(), EvalError> {
        let func = "union_agg";
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut all = std::mem::take(&mut self.pending);
        all.extend(self.union.take());
        let collection = geo::Geometry::GeometryCollection(geo::GeometryCollection(all));
        let union = to_geos(func, &collection)?
            .unary_union()
            .map_err(|e| invalid(func, e))?;
        self.union = Some(geo::Geometry::try_from(union).map_err(|e| invalid(func, e))?);
        Ok(())
    }
}

impl Accumulator for UnionAgg {
    fn update(&mut self, args: &[Value]) -> Result<(), EvalError> {
        if args[0].is_null() {
            return Ok(());
        }
        self.pending.push(geometry("union_agg", &args[0])?);
        if self.pending.len() >= UNION_BATCH {
            self.collapse()?;
        }
        Ok(())
    }

    fn merge(&mut self, other: &dyn Accumulator) {
        let other = same::<UnionAgg>(other);
        self.pending.extend(other.union.iter().cloned());
        self.pending.extend(other.pending.iter().cloned());
    }

    fn finish(&self) -> Result<Value, EvalError> {
        let mut all = UnionAgg {
            union: self.union.clone(),
            pending: self.pending.clone(),
        };
        all.collapse()?;
        match all.union {
            Some(g) if !g.is_empty() => Ok(to_json(&g)),
            _ => Ok(Value::Null),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Value {
        json!({"type": "Polygon", "coordinates": [[
            [x, y], [x + size, y], [x + size, y + size], [x, y + size], [x, y]
        ]]})
    }

    fn eval(src: &str, record: Value) -> Result<Value, EvalError> {
        registry()
            .compile(src)
            .unwrap_or_else(|e| panic!("{}: {}", src, e))
            .eval(&record)
    }

    fn f64_of(v: Result<Value, EvalError>) -> f64 {
        v.unwrap().as_f64().expect("a number")
    }

    #[test]
    fn test_measures() {
        let r = json!({"g": square(0.0, 0.0, 2.0), "l": {"type": "LineString", "coordinates": [[0, 0], [3, 4]]}});
        assert_eq!(eval("planar_area(d.g)", r.clone()), Ok(json!(4.0)));
        assert_eq!(eval("planar_length(d.g)", r.clone()), Ok(json!(8.0)));
        assert_eq!(eval("planar_length(d.l)", r.clone()), Ok(json!(5.0)));
        assert_eq!(eval("planar_area(d.l)", r.clone()), Ok(json!(0.0)));
        assert_eq!(eval("num_points(d.g)", r.clone()), Ok(json!(5)));
        assert_eq!(
            eval("bbox(d.l)", r.clone()),
            Ok(json!([0.0, 0.0, 3.0, 4.0]))
        );
        assert_eq!(
            eval("centroid(d.g)", r.clone()),
            Ok(json!({"type": "Point", "coordinates": [1.0, 1.0]}))
        );
        assert_eq!(eval("geom_type(d.l)", r.clone()), Ok(json!("LineString")));
        assert_eq!(eval("is_empty(d.g)", r), Ok(json!(false)));
    }

    #[test]
    fn test_geodesic_measures() {
        // one degree of longitude on the equator is about 111.3 km
        let line = json!({"type": "LineString", "coordinates": [[0, 0], [1, 0]]});
        let m = f64_of(eval("geodesic_length(d)", line));
        assert!((m - 111_319.5).abs() < 1.0, "{}", m);
        let a = f64_of(eval("geodesic_area(d)", square(0.0, 0.0, 1.0)));
        assert!((a - 12_308_778_361.0).abs() < 1e7, "{}", a);
        assert_eq!(eval("area(d)", square(0.0, 0.0, 1.0)), Ok(json!(a)));
        let line = json!({"type": "LineString", "coordinates": [[0, 0], [1, 0]]});
        assert_eq!(eval("length(d)", line), Ok(json!(m)));
    }

    #[test]
    fn test_geodesic_area_of_clockwise_rings() {
        // a ~105 m by ~390 m block in Minneapolis, written clockwise
        let cw = json!({"type": "Polygon", "coordinates": [[
            [-93.30807, 44.91183], [-93.30793, 44.90833], [-93.30933, 44.90833],
            [-93.30933, 44.91183], [-93.30807, 44.91183]
        ]]});
        let a = f64_of(eval("area(d)", cw.clone()));
        assert!((a - 40_852.0).abs() < 1.0, "{}", a);
        let multi = json!({"type": "MultiPolygon", "coordinates": [cw["coordinates"]]});
        let b = f64_of(eval("area(d)", multi));
        assert!((a - b).abs() < 1e-6, "{} {}", a, b);
    }

    #[test]
    fn test_features_and_bad_arguments() {
        let feature =
            json!({"type": "Feature", "geometry": square(0.0, 0.0, 1.0), "properties": {}});
        assert_eq!(eval("planar_area(d)", feature), Ok(json!(1.0)));
        assert_eq!(eval("area(d.missing)", json!({})), Ok(Value::Null));
        assert_eq!(
            eval("area(d.s)", json!({"s": "x"})),
            Err(EvalError::Type {
                op: "area",
                left: "string",
                right: None
            })
        );
        assert!(matches!(
            eval("area(d)", json!({"type": "Polygon", "coordinates": 1})),
            Err(EvalError::Invalid { func: "area", .. })
        ));
        let no_geometry = json!({"type": "Feature", "geometry": null, "properties": {}});
        assert_eq!(
            eval("area(d)", no_geometry).unwrap_err().to_string(),
            "area(): feature has no geometry"
        );
    }

    #[test]
    fn test_coordinates_of_points_only() {
        let p = json!({"type": "Point", "coordinates": [3.5, -2]});
        assert_eq!(eval("x(d)", p.clone()), Ok(json!(3.5)));
        assert_eq!(eval("y(d)", p), Ok(json!(-2.0)));
        assert_eq!(
            eval("x(d)", square(0.0, 0.0, 1.0)).unwrap_err().to_string(),
            "x(): expected a Point, got a Polygon"
        );
    }

    #[test]
    fn test_predicates_against_wkt_or_geometry() {
        let r = json!({"g": square(0.0, 0.0, 2.0), "p": {"type": "Point", "coordinates": [1, 1]}});
        assert_eq!(
            eval("contains(d.g, \"POINT (1 1)\")", r.clone()),
            Ok(json!(true))
        );
        assert_eq!(eval("contains(d.g, d.p)", r.clone()), Ok(json!(true)));
        assert_eq!(eval("within(d.p, d.g)", r.clone()), Ok(json!(true)));
        assert_eq!(eval("within(d.g, d.p)", r.clone()), Ok(json!(false)));
        assert_eq!(
            eval("intersects(d.g, \"LINESTRING (1 1, 9 9)\")", r.clone()),
            Ok(json!(true))
        );
        assert_eq!(
            eval("distance(d.g, \"POINT (5 2)\")", r.clone()),
            Ok(json!(3.0))
        );
        // a different literal on the next call isn't served from the cache
        assert_eq!(
            eval("distance(d.g, \"POINT (2 6)\")", r.clone()),
            Ok(json!(4.0))
        );
        assert!(matches!(
            eval("contains(d.g, \"POINT (1\")", r),
            Err(EvalError::Invalid {
                func: "contains",
                ..
            })
        ));
    }

    #[test]
    fn test_geometry_operations() {
        let r = json!({"g": square(0.0, 0.0, 2.0)});
        assert_eq!(
            eval("to_wkt(d.g)", r.clone()),
            Ok(json!("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"))
        );
        assert_eq!(
            eval("planar_area(from_wkt(to_wkt(d.g)))", r.clone()),
            Ok(json!(4.0))
        );
        let buffered = f64_of(eval(
            "planar_area(buffer(from_wkt(\"POINT (0 0)\"), 1))",
            r.clone(),
        ));
        assert!(
            (buffered - std::f64::consts::PI).abs() < 0.05,
            "{}",
            buffered
        );
        assert_eq!(
            eval(
                "planar_area(convex_hull(from_wkt(\"MULTIPOINT (0 0, 2 0, 0 2, 2 2, 1 1)\")))",
                r.clone()
            ),
            Ok(json!(4.0))
        );
        assert_eq!(
            eval(
                "num_points(simplify(from_wkt(\"LINESTRING (0 0, 1 0.01, 2 0)\"), 0.1))",
                r.clone()
            ),
            Ok(json!(2))
        );
        let bowtie = "from_wkt(\"POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))\")";
        assert_eq!(
            eval(&format!("is_valid({})", bowtie), r.clone()),
            Ok(json!(false))
        );
        assert_eq!(
            eval(&format!("is_valid(make_valid({}))", bowtie), r.clone()),
            Ok(json!(true))
        );
        assert_eq!(
            eval("geom_type(point_on_surface(d.g))", r.clone()),
            Ok(json!("Point"))
        );
        assert_eq!(eval("from_wkt(\"POINT EMPTY\")", r), Ok(Value::Null));
    }

    #[test]
    fn test_transform() {
        let p = json!({"type": "Point", "coordinates": [-93.0, 45.0]});
        let out = eval("transform(d, \"EPSG:4326\", \"EPSG:3857\")", p.clone()).unwrap();
        let c = &out["coordinates"];
        assert!(
            (c[0].as_f64().unwrap() - -10_352_712.64).abs() < 1.0,
            "{}",
            out
        );
        assert!(
            (c[1].as_f64().unwrap() - 5_621_521.49).abs() < 1.0,
            "{}",
            out
        );
        assert!(matches!(
            eval("transform(d, \"EPSG:4326\", \"nope\")", p),
            Err(EvalError::Invalid {
                func: "transform",
                ..
            })
        ));
    }

    #[test]
    fn test_cells() {
        let p = |x: f64, y: f64| json!({"type": "Point", "coordinates": [x, y]});
        assert_eq!(eval("geohash(d, 5)", p(-5.6, 42.6)), Ok(json!("ezs42")));
        // Bing's example tile x=3, y=5 at level 3
        assert_eq!(eval("quadkey(d, 3)", p(-22.5, -55.8)), Ok(json!("213")));
        assert_eq!(eval("quadkey(d, 1)", p(10.0, 10.0)), Ok(json!("1")));
        // the centroid is used for other geometries
        assert_eq!(
            eval("geohash(d, 5)", square(-5.7, 42.5, 0.2)),
            Ok(json!("ezs42"))
        );
        // the example in the H3 docs
        assert_eq!(
            eval("h3(d, 9)", p(-122.418307270836, 37.7752702151959)),
            Ok(json!("8928308280fffff"))
        );
        assert_eq!(
            eval("geohash(d, 13)", p(0.0, 0.0)).unwrap_err().to_string(),
            "geohash(): precision must be an integer from 0 to 12"
        );
        assert!(eval("h3(d, -1)", p(0.0, 0.0)).is_err());
    }

    #[test]
    fn test_extent_and_centroid_agg() {
        let mut extent = Extent::default();
        extent.update(&[square(0.0, 0.0, 1.0)]).unwrap();
        extent.update(&[Value::Null]).unwrap();
        let mut other = Extent::default();
        other.update(&[square(-3.0, 2.0, 1.0)]).unwrap();
        extent.merge(&other);
        assert_eq!(extent.finish(), Ok(json!([-3.0, 0.0, 1.0, 3.0])));
        assert_eq!(Extent::default().finish(), Ok(Value::Null));

        let mut centroid = CentroidAgg::default();
        // the point is ignored once there are polygons
        centroid
            .update(&[json!({"type": "Point", "coordinates": [100, 100]})])
            .unwrap();
        centroid.update(&[square(0.0, 0.0, 2.0)]).unwrap();
        let mut other = CentroidAgg::default();
        // weighted by area: 4 at (1, 1) and 1 at (6.5, 0.5)
        other.update(&[square(6.0, 0.0, 1.0)]).unwrap();
        centroid.merge(&other);
        assert_eq!(
            centroid.finish(),
            Ok(json!({"type": "Point", "coordinates": [2.1, 0.9]}))
        );
    }

    #[test]
    fn test_union_agg() {
        let registry = registry();
        assert!(registry.get("union_agg").unwrap().is_unbounded());
        assert!(!registry.get("extent").unwrap().is_unbounded());
        let mut union = UnionAgg::default();
        for i in 0..(UNION_BATCH + 10) {
            union.update(&[square(i as f64, 0.0, 2.0)]).unwrap();
        }
        let mut other = UnionAgg::default();
        other.update(&[square(-1.0, 0.0, 1.0)]).unwrap();
        union.merge(&other);
        let g = geometry("test", &union.finish().unwrap()).unwrap();
        assert_eq!(g.unsigned_area(), 2.0 * (UNION_BATCH + 11) as f64 + 1.0);
    }
}
