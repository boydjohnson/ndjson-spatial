pub mod ast;

use lalrpop_util::lalrpop_mod;

lalrpop_mod!(pub math);

pub use self::math::ExprParser;

use self::ast::{Expr, Num, Term, Value};
use ordered_float::OrderedFloat;

/// Negate a numeric literal. Integers that fit stay integers; `-0` stays
/// `PosInt(0)`; magnitudes beyond `i64::MIN` become floats.
pub(crate) fn negate_num(n: Num) -> Num {
    match n {
        Num::PosInt(0) => Num::PosInt(0),
        Num::PosInt(p) if p <= 1 << 63 => Num::Int((-(p as i128)) as i64),
        Num::PosInt(p) => Num::Float(OrderedFloat(-(p as f64))),
        Num::Int(i) => match i.checked_neg() {
            Some(v) if v >= 0 => Num::PosInt(v as u64),
            Some(v) => Num::Int(v),
            None => Num::PosInt(1 << 63),
        },
        Num::Float(f) => Num::Float(OrderedFloat(-f.0)),
    }
}

/// Unary minus: fold into numeric literals, otherwise wrap.
#[allow(clippy::boxed_local)] // the grammar hands us a Box
pub(crate) fn negate(e: Box<Expr>) -> Box<Expr> {
    match *e {
        Expr::Term(Term::Value(Value::Num(n))) => {
            Box::new(Expr::Term(Term::Value(Value::Num(negate_num(n)))))
        }
        other => Box::new(Expr::Negate(Box::new(other))),
    }
}

/// Resolve backslash escapes in a string literal or quoted key (without the
/// surrounding quotes). Unknown escapes keep the escaped character.
pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::math::{ExprParser, FloatParser, IntParser, NumParser, PosIntParser, TermParser};
    use crate::parser::ast::{BinaryOp, Expr, Num, Term, Value};
    use ordered_float::OrderedFloat;
    use yajlish_ndjson::Selector;

    #[test]
    fn test_float_parser() {
        assert_eq!(FloatParser::new().parse("4.5"), Ok(4.5));

        assert_eq!(FloatParser::new().parse("0.55546"), Ok(0.55546));

        assert_eq!(FloatParser::new().parse("45e2"), Ok(4500.0));

        assert_eq!(FloatParser::new().parse("4676e-2"), Ok(46.76));
        assert!(FloatParser::new().parse("345-443").is_err());
    }

    #[test]
    fn test_posint_parser() {
        assert_eq!(PosIntParser::new().parse("45643948"), Ok(45643948));

        assert!(PosIntParser::new()
            .parse("284849585850000000000000")
            .is_err());

        assert!(PosIntParser::new().parse("44858---00000").is_err());
    }

    #[test]
    fn test_int_parser() {
        assert_eq!(IntParser::new().parse("-345454"), Ok(-345454));
        assert_eq!(IntParser::new().parse("-10"), Ok(-10));

        assert!(IntParser::new().parse("-10.0").is_err());
        assert!(IntParser::new().parse("--10.4").is_err());
    }

    #[test]
    fn test_num_parser() {
        assert_eq!(NumParser::new().parse("4564345"), Ok(Num::PosInt(4564345)));
        assert_eq!(
            NumParser::new().parse("0.0456400"),
            Ok(Num::Float(OrderedFloat(0.04564)))
        );
        assert_eq!(
            NumParser::new().parse("345e-3"),
            Ok(Num::Float(OrderedFloat(0.345)))
        );
        assert_eq!(NumParser::new().parse("-345"), Ok(Num::Int(-345)));

        assert!(NumParser::new().parse("-345-").is_err());
    }

    #[test]
    fn test_term_parser() {
        assert_eq!(
            TermParser::new().parse("d.manager.salary"),
            Ok(Box::new(Expr::Term(Term::Selector(vec![
                Selector::Identifier("manager".into()),
                Selector::Identifier("salary".into())
            ])))),
        );

        assert_eq!(
            TermParser::new().parse("true"),
            Ok(Box::new(Expr::Term(Term::Value(Value::Bool(true)))))
        );
    }

    #[test]
    fn test_expr_parser() {
        assert_eq!(
            ExprParser::new().parse("7 + 5 * 9"),
            Ok(Box::new(Expr::Op(
                Box::new(Expr::Term(Term::Value(Value::Num(Num::PosInt(7))))),
                BinaryOp::Add,
                Box::new(Expr::Op(
                    Box::new(Expr::Term(Term::Value(Value::Num(Num::PosInt(5))))),
                    BinaryOp::Mul,
                    Box::new(Expr::Term(Term::Value(Value::Num(Num::PosInt(9)))))
                ))
            ))),
        );

        assert_eq!(
            ExprParser::new().parse("(d.TOTAL_SALES + 5) / \"dough\""),
            Ok(Box::new(Expr::Op(
                Box::new(Expr::Op(
                    Box::new(Expr::Term(Term::Selector(vec![Selector::Identifier(
                        "TOTAL_SALES".into()
                    )]))),
                    BinaryOp::Add,
                    Box::new(Expr::Term(Term::Value(Value::Num(Num::PosInt(5)))))
                )),
                BinaryOp::Div,
                Box::new(Expr::Term(Term::Value(Value::String("dough".into())))),
            ))),
        );

        assert_eq!(
            ExprParser::new().parse("(3.4 + 5) ** 5 / 4.5"),
            Ok(Box::new(Expr::Op(
                Box::new(Expr::Op(
                    Box::new(Expr::Op(
                        Box::new(Expr::Term(Term::Value(Value::Num(Num::Float(
                            OrderedFloat(3.4)
                        ))))),
                        BinaryOp::Add,
                        Box::new(Expr::Term(Term::Value(Value::Num(Num::PosInt(5)))))
                    )),
                    BinaryOp::Pow,
                    Box::new(Expr::Term(Term::Value(Value::Num(Num::PosInt(5)))))
                )),
                BinaryOp::Div,
                Box::new(Expr::Term(Term::Value(Value::Num(Num::Float(
                    OrderedFloat(4.5)
                )))))
            )))
        );

        assert!(ExprParser::new()
            .parse("(d.TOTAL_SALES + ) / false")
            .is_err());
    }

    fn call(name: &str, args: Vec<Box<Expr>>) -> Box<Expr> {
        Box::new(Expr::Call(
            name.into(),
            args.into_iter().map(|a| *a).collect(),
        ))
    }

    #[test]
    fn test_expr_with_function_calls() {
        assert_eq!(
            ExprParser::new().parse("toint(3.4)"),
            Ok(call("toint", vec![num(Num::Float(OrderedFloat(3.4)))]))
        );

        assert_eq!(
            ExprParser::new().parse("ceil(4.5) + floor(4.5)"),
            Ok(Box::new(Expr::Op(
                call("ceil", vec![num(Num::Float(OrderedFloat(4.5)))]),
                BinaryOp::Add,
                call("floor", vec![num(Num::Float(OrderedFloat(4.5)))])
            )))
        );
    }

    #[test]
    fn test_calls_take_any_number_of_arguments() {
        assert_eq!(ExprParser::new().parse("now()"), Ok(call("now", vec![])));
        assert_eq!(
            ExprParser::new().parse("round(d.x, 2)"),
            Ok(call("round", vec![sel(&["x"]), num(Num::PosInt(2))]))
        );
        assert_eq!(
            ExprParser::new().parse("f(1, g(d.a + 1), \"s\")"),
            Ok(call(
                "f",
                vec![
                    num(Num::PosInt(1)),
                    call(
                        "g",
                        vec![Box::new(Expr::Op(
                            sel(&["a"]),
                            BinaryOp::Add,
                            num(Num::PosInt(1))
                        ))]
                    ),
                    string("s"),
                ]
            ))
        );
        // Names are parsed without knowing which functions exist.
        assert_eq!(
            ExprParser::new().parse("geom_type2(d)"),
            Ok(Box::new(Expr::Call(
                "geom_type2".into(),
                vec![Expr::Term(Term::Selector(vec![]))]
            )))
        );
        assert!(ExprParser::new().parse("f(,)").is_err());
        assert!(ExprParser::new().parse("f(1 2)").is_err());
        assert!(ExprParser::new().parse("f").is_err());
    }

    #[test]
    fn test_keywords_are_not_function_names() {
        assert_eq!(
            ExprParser::new().parse("true"),
            Ok(Box::new(Expr::Term(Term::Value(Value::Bool(true)))))
        );
        assert_eq!(
            ExprParser::new().parse("Null"),
            Ok(Box::new(Expr::Term(Term::Value(Value::Null))))
        );
        assert_eq!(
            ExprParser::new().parse("trueish(null)"),
            Ok(call(
                "trueish",
                vec![Box::new(Expr::Term(Term::Value(Value::Null)))]
            ))
        );
        assert_eq!(
            ExprParser::new().parse("data(d.a)"),
            Ok(call("data", vec![sel(&["a"])]))
        );
    }

    fn num(n: Num) -> Box<Expr> {
        Box::new(Expr::Term(Term::Value(Value::Num(n))))
    }

    fn sel(path: &[&str]) -> Box<Expr> {
        Box::new(Expr::Term(Term::Selector(
            path.iter()
                .map(|p| Selector::Identifier((*p).into()))
                .collect(),
        )))
    }

    fn string(s: &str) -> Box<Expr> {
        Box::new(Expr::Term(Term::Value(Value::String(s.into()))))
    }

    #[test]
    fn test_identifier_stops_at_punctuation() {
        assert_eq!(
            ExprParser::new().parse("abs(d.amount)"),
            Ok(call("abs", vec![sel(&["amount"])]))
        );
        assert_eq!(
            ExprParser::new().parse("(d.a)*d.b"),
            Ok(Box::new(Expr::Op(sel(&["a"]), BinaryOp::Mul, sel(&["b"]))))
        );
    }

    #[test]
    fn test_quoted_and_numeric_keys() {
        assert_eq!(
            ExprParser::new().parse(r#"d."key with spaces"[0]["other.key"].2020"#),
            Ok(Box::new(Expr::Term(Term::Selector(vec![
                Selector::Identifier("key with spaces".into()),
                Selector::Index(0),
                Selector::Identifier("other.key".into()),
                Selector::Identifier("2020".into()),
            ]))))
        );
    }

    #[test]
    fn test_strings_are_not_greedy() {
        assert_eq!(
            ExprParser::new().parse(r#""a" + "b""#),
            Ok(Box::new(Expr::Op(string("a"), BinaryOp::Add, string("b"))))
        );
        assert_eq!(
            ExprParser::new().parse(r#""say \"hi\"\n""#),
            Ok(string("say \"hi\"\n"))
        );
    }

    #[test]
    fn test_minus_without_spaces_is_subtraction() {
        assert_eq!(
            ExprParser::new().parse("3-5"),
            Ok(Box::new(Expr::Op(
                num(Num::PosInt(3)),
                BinaryOp::Sub,
                num(Num::PosInt(5))
            )))
        );
        assert_eq!(
            ExprParser::new().parse("d.a-1"),
            Ok(Box::new(Expr::Op(
                sel(&["a"]),
                BinaryOp::Sub,
                num(Num::PosInt(1))
            )))
        );
    }

    #[test]
    fn test_unary_minus() {
        assert_eq!(
            ExprParser::new().parse("2 - -3"),
            Ok(Box::new(Expr::Op(
                num(Num::PosInt(2)),
                BinaryOp::Sub,
                num(Num::Int(-3))
            )))
        );
        assert_eq!(
            ExprParser::new().parse("-d.x"),
            Ok(Box::new(Expr::Negate(sel(&["x"]))))
        );
        // Python precedence: -2 ** 2 == -(2 ** 2)
        assert_eq!(
            ExprParser::new().parse("-2 ** 2"),
            Ok(Box::new(Expr::Negate(Box::new(Expr::Op(
                num(Num::PosInt(2)),
                BinaryOp::Pow,
                num(Num::PosInt(2))
            )))))
        );
        assert_eq!(
            ExprParser::new().parse("-9223372036854775808"),
            Ok(num(Num::Int(i64::MIN)))
        );
        assert_eq!(ExprParser::new().parse("--4"), Ok(num(Num::PosInt(4))));
        assert_eq!(
            ExprParser::new().parse("-1.5"),
            Ok(num(Num::Float(OrderedFloat(-1.5))))
        );
    }

    #[test]
    fn test_float_forms() {
        assert_eq!(FloatParser::new().parse("1.5e3"), Ok(1500.0));
        assert_eq!(FloatParser::new().parse("1E5"), Ok(100000.0));
        assert_eq!(FloatParser::new().parse("2e+2"), Ok(200.0));
        assert_eq!(FloatParser::new().parse("2.5E-1"), Ok(0.25));
    }
}
