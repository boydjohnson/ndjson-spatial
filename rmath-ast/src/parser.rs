pub mod ast;

use lalrpop_util::lalrpop_mod;

lalrpop_mod!(pub math);

pub use self::math::ExprParser;

#[cfg(test)]
mod tests {
    use super::math::{ExprParser, FloatParser, IntParser, NumParser, PosIntParser, TermParser};
    use crate::parser::ast::{BinaryOp, Expr, Num, Term, UnaryFunction, Value};
    use ordered_float::OrderedFloat;
    use yajlish::ndjson_handler::Selector;

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

    #[test]
    fn test_expr_with_unary_functions() {
        assert_eq!(
            ExprParser::new().parse("toint(3.4)"),
            Ok(Box::new(Expr::UnaryFunction(
                UnaryFunction::ToInt,
                Box::new(Expr::Term(Term::Value(Value::Num(Num::Float(
                    OrderedFloat(3.4)
                )))))
            )))
        );

        assert_eq!(
            ExprParser::new().parse("ceil(4.5) + floor(4.5)"),
            Ok(Box::new(Expr::Op(
                Box::new(Expr::UnaryFunction(
                    UnaryFunction::Ceil,
                    Box::new(Expr::Term(Term::Value(Value::Num(Num::Float(
                        OrderedFloat(4.5)
                    )))))
                )),
                BinaryOp::Add,
                Box::new(Expr::UnaryFunction(
                    UnaryFunction::Floor,
                    Box::new(Expr::Term(Term::Value(Value::Num(Num::Float(
                        OrderedFloat(4.5)
                    )))))
                ))
            )))
        );
    }
}
