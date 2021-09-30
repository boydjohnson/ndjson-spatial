use serde_json::Value;

pub fn infer_json(item: &str) -> Value {
    if let Ok(possible) = item.parse::<u64>() {
        if possible.to_string() == item {
            return possible.into();
        }
    }
    if let Ok(possible) = item.parse::<i64>() {
        if possible.to_string() == item {
            return possible.into();
        }
    }
    if let Ok(possible) = item.parse::<f64>() {
        return possible.into();
    }
    if item.is_empty() {
        return Value::Null;
    }
    if let Ok(possible) = item.parse::<bool>() {
        if possible.to_string() == item {
            return possible.into();
        }
    }
    item.to_string().into()
}
