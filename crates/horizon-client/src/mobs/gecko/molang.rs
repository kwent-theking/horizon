//! Small Molang evaluator for GeckoLib animation expressions.
//!
//! Supported: numbers, `+ - * /`, unary `-`/`!`, comparisons, `&& ||`, ternary
//! `a ? b : c`, dotted names (`query.x`, `q.x`, `variable.x`, `v.x`), bare names
//! (unknown names evaluate to 0), `math.*` functions, and `;`-separated
//! statements with an optional `return`. Division by zero yields 0.
//! `math.sin`/`math.cos` take degrees, as Bedrock does.

use std::f32::consts::PI;

pub trait MolangContext {
    fn query(&self, name: &str) -> f32;
}

#[derive(Debug, Clone)]
pub struct Expr {
    statements: Vec<Statement>,
}

#[derive(Debug, Clone)]
enum Statement {
    Return(Node),
    Evaluate(Node),
}

#[derive(Debug, Clone)]
enum Node {
    Number(f32),
    Query(Box<str>),
    Negate(Box<Node>),
    Not(Box<Node>),
    Binary(BinOp, Box<Node>, Box<Node>),
    Ternary(Box<Node>, Box<Node>, Box<Node>),
    Call(Func, Vec<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Abs,
    Clamp,
    Lerp,
    Pow,
    Sqrt,
    Floor,
    Ceil,
    Round,
    Min,
    Max,
    Mod,
}

impl Func {
    fn lookup(name: &str) -> Option<(Func, usize)> {
        Some(match name {
            "math.sin" => (Func::Sin, 1),
            "math.cos" => (Func::Cos, 1),
            "math.abs" => (Func::Abs, 1),
            "math.clamp" => (Func::Clamp, 3),
            "math.lerp" => (Func::Lerp, 3),
            "math.pow" => (Func::Pow, 2),
            "math.sqrt" => (Func::Sqrt, 1),
            "math.floor" => (Func::Floor, 1),
            "math.ceil" => (Func::Ceil, 1),
            "math.round" => (Func::Round, 1),
            "math.min" => (Func::Min, 2),
            "math.max" => (Func::Max, 2),
            "math.mod" => (Func::Mod, 2),
            _ => return None,
        })
    }

    fn apply(self, args: &[f32]) -> f32 {
        match self {
            Func::Sin => args[0].to_radians().sin(),
            Func::Cos => args[0].to_radians().cos(),
            Func::Abs => args[0].abs(),
            Func::Clamp => args[0].max(args[1]).min(args[2]),
            Func::Lerp => args[0] + (args[1] - args[0]) * args[2],
            Func::Pow => args[0].powf(args[1]),
            Func::Sqrt => args[0].sqrt(),
            Func::Floor => args[0].floor(),
            Func::Ceil => args[0].ceil(),
            Func::Round => args[0].round(),
            Func::Min => args[0].min(args[1]),
            Func::Max => args[0].max(args[1]),
            Func::Mod => remainder_or_zero(args[0], args[1]),
        }
    }
}

impl Expr {
    pub fn eval(&self, ctx: &dyn MolangContext) -> f32 {
        let mut last = 0.0;
        for statement in &self.statements {
            match statement {
                Statement::Return(node) => return eval_node(node, ctx),
                Statement::Evaluate(node) => last = eval_node(node, ctx),
            }
        }
        last
    }
}

pub fn compile(src: &str) -> Result<Expr, String> {
    let tokens = tokenize(src)?;
    Parser { tokens, pos: 0 }.parse_program()
}

fn eval_node(node: &Node, ctx: &dyn MolangContext) -> f32 {
    match node {
        Node::Number(value) => *value,
        Node::Query(name) => ctx.query(name),
        Node::Negate(inner) => -eval_node(inner, ctx),
        Node::Not(inner) => bool_value(!is_truthy(eval_node(inner, ctx))),
        Node::Binary(op, lhs, rhs) => eval_binary(*op, lhs, rhs, ctx),
        Node::Ternary(cond, yes, no) => {
            if is_truthy(eval_node(cond, ctx)) {
                eval_node(yes, ctx)
            } else {
                eval_node(no, ctx)
            }
        }
        Node::Call(func, args) => eval_call(*func, args, ctx),
    }
}

fn eval_binary(op: BinOp, lhs: &Node, rhs: &Node, ctx: &dyn MolangContext) -> f32 {
    match op {
        BinOp::And => bool_value(is_truthy(eval_node(lhs, ctx)) && is_truthy(eval_node(rhs, ctx))),
        BinOp::Or => bool_value(is_truthy(eval_node(lhs, ctx)) || is_truthy(eval_node(rhs, ctx))),
        _ => apply_arithmetic(op, eval_node(lhs, ctx), eval_node(rhs, ctx)),
    }
}

fn apply_arithmetic(op: BinOp, a: f32, b: f32) -> f32 {
    match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => divide_or_zero(a, b),
        BinOp::Lt => bool_value(a < b),
        BinOp::Le => bool_value(a <= b),
        BinOp::Gt => bool_value(a > b),
        BinOp::Ge => bool_value(a >= b),
        BinOp::Eq => bool_value(a == b),
        BinOp::Ne => bool_value(a != b),
        BinOp::And | BinOp::Or => {
            unreachable!("logical operators are short-circuited in eval_binary")
        }
    }
}

fn eval_call(func: Func, args: &[Node], ctx: &dyn MolangContext) -> f32 {
    let mut values = [0.0f32; 3];
    for (slot, arg) in values.iter_mut().zip(args) {
        *slot = eval_node(arg, ctx);
    }
    func.apply(&values[..args.len()])
}

fn divide_or_zero(a: f32, b: f32) -> f32 {
    if b == 0.0 { 0.0 } else { a / b }
}

fn remainder_or_zero(a: f32, b: f32) -> f32 {
    if b == 0.0 { 0.0 } else { a % b }
}

fn is_truthy(value: f32) -> bool {
    value != 0.0
}

fn bool_value(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f32),
    Name(String),
    Return,
    Plus,
    Minus,
    Star,
    Slash,
    Bang,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    NotEq,
    AndAnd,
    OrOr,
    LParen,
    RParen,
    Comma,
    Question,
    Colon,
    Semi,
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte.is_ascii_whitespace() {
            i += 1;
        } else if byte.is_ascii_digit() || (byte == b'.' && next_is_digit(bytes, i)) {
            let end = scan_number(bytes, i);
            let value = src[i..end]
                .parse::<f32>()
                .map_err(|_| format!("invalid number '{}'", &src[i..end]))?;
            tokens.push(Token::Number(value));
            i = end;
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let end = scan_name(bytes, i);
            let name = src[i..end].to_ascii_lowercase();
            tokens.push(if name == "return" {
                Token::Return
            } else {
                Token::Name(name)
            });
            i = end;
        } else {
            let (token, width) = punctuation(bytes, i)?;
            tokens.push(token);
            i += width;
        }
    }
    Ok(tokens)
}

fn punctuation(bytes: &[u8], i: usize) -> Result<(Token, usize), String> {
    let next = bytes.get(i + 1).copied();
    let pair = match (bytes[i], next) {
        (b'<', Some(b'=')) => Some(Token::Le),
        (b'>', Some(b'=')) => Some(Token::Ge),
        (b'=', Some(b'=')) => Some(Token::EqEq),
        (b'!', Some(b'=')) => Some(Token::NotEq),
        (b'&', Some(b'&')) => Some(Token::AndAnd),
        (b'|', Some(b'|')) => Some(Token::OrOr),
        _ => None,
    };
    if let Some(token) = pair {
        return Ok((token, 2));
    }
    let single = match bytes[i] {
        b'<' => Token::Lt,
        b'>' => Token::Gt,
        b'!' => Token::Bang,
        b'+' => Token::Plus,
        b'-' => Token::Minus,
        b'*' => Token::Star,
        b'/' => Token::Slash,
        b'(' => Token::LParen,
        b')' => Token::RParen,
        b',' => Token::Comma,
        b'?' => Token::Question,
        b':' => Token::Colon,
        b';' => Token::Semi,
        other => {
            return Err(format!(
                "unexpected character '{}' at byte {i}",
                other as char
            ));
        }
    };
    Ok((single, 1))
}

fn next_is_digit(bytes: &[u8], i: usize) -> bool {
    bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
}

fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut end = skip_digits(bytes, start);
    if bytes.get(end) == Some(&b'.') {
        end = skip_digits(bytes, end + 1);
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let digits_end = skip_digits(bytes, exponent);
        if digits_end > exponent {
            end = digits_end;
        }
    }
    end
}

fn skip_digits(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    i
}

fn scan_name(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')
    {
        end += 1;
    }
    end
}

type Operand = fn(&mut Parser) -> Result<Node, String>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn parse_program(mut self) -> Result<Expr, String> {
        let mut statements = Vec::new();
        loop {
            while self.eat(&Token::Semi) {}
            if self.peek().is_none() {
                break;
            }
            let statement = if self.eat(&Token::Return) {
                Statement::Return(self.parse_ternary()?)
            } else {
                Statement::Evaluate(self.parse_ternary()?)
            };
            statements.push(statement);
            if self.peek().is_some() {
                self.expect(&Token::Semi, "';'")?;
            }
        }
        Ok(Expr { statements })
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token, description: &str) -> Result<(), String> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(format!("expected {description} at token {}", self.pos))
        }
    }

    fn parse_ternary(&mut self) -> Result<Node, String> {
        let condition = self.parse_or()?;
        if !self.eat(&Token::Question) {
            return Ok(condition);
        }
        let yes = self.parse_ternary()?;
        self.expect(&Token::Colon, "':'")?;
        let no = self.parse_ternary()?;
        Ok(Node::Ternary(
            Box::new(condition),
            Box::new(yes),
            Box::new(no),
        ))
    }

    fn parse_or(&mut self) -> Result<Node, String> {
        self.parse_left_assoc(Self::parse_and, &[(Token::OrOr, BinOp::Or)])
    }

    fn parse_and(&mut self) -> Result<Node, String> {
        self.parse_left_assoc(Self::parse_equality, &[(Token::AndAnd, BinOp::And)])
    }

    fn parse_equality(&mut self) -> Result<Node, String> {
        self.parse_left_assoc(
            Self::parse_relational,
            &[(Token::EqEq, BinOp::Eq), (Token::NotEq, BinOp::Ne)],
        )
    }

    fn parse_relational(&mut self) -> Result<Node, String> {
        self.parse_left_assoc(
            Self::parse_additive,
            &[
                (Token::Lt, BinOp::Lt),
                (Token::Le, BinOp::Le),
                (Token::Gt, BinOp::Gt),
                (Token::Ge, BinOp::Ge),
            ],
        )
    }

    fn parse_additive(&mut self) -> Result<Node, String> {
        self.parse_left_assoc(
            Self::parse_multiplicative,
            &[(Token::Plus, BinOp::Add), (Token::Minus, BinOp::Sub)],
        )
    }

    fn parse_multiplicative(&mut self) -> Result<Node, String> {
        self.parse_left_assoc(
            Self::parse_unary,
            &[(Token::Star, BinOp::Mul), (Token::Slash, BinOp::Div)],
        )
    }

    fn parse_left_assoc(
        &mut self,
        operand: Operand,
        operators: &[(Token, BinOp)],
    ) -> Result<Node, String> {
        let mut node = operand(self)?;
        while let Some(op) = self.take_operator(operators) {
            let rhs = operand(self)?;
            node = Node::Binary(op, Box::new(node), Box::new(rhs));
        }
        Ok(node)
    }

    fn take_operator(&mut self, operators: &[(Token, BinOp)]) -> Option<BinOp> {
        let (_, op) = operators
            .iter()
            .find(|(token, _)| self.peek() == Some(token))?;
        self.pos += 1;
        Some(*op)
    }

    fn parse_unary(&mut self) -> Result<Node, String> {
        if self.eat(&Token::Minus) {
            return Ok(Node::Negate(Box::new(self.parse_unary()?)));
        }
        if self.eat(&Token::Bang) {
            return Ok(Node::Not(Box::new(self.parse_unary()?)));
        }
        if self.eat(&Token::Plus) {
            return self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Node, String> {
        match self.advance() {
            Some(Token::Number(value)) => Ok(Node::Number(value)),
            Some(Token::LParen) => {
                let inner = self.parse_ternary()?;
                self.expect(&Token::RParen, "')'")?;
                Ok(inner)
            }
            Some(Token::Name(name)) => self.parse_name(normalize_name(name)),
            Some(other) => Err(format!("unexpected token {other:?}")),
            None => Err("unexpected end of expression".to_string()),
        }
    }

    fn parse_name(&mut self, name: String) -> Result<Node, String> {
        if self.eat(&Token::LParen) {
            let (func, arity) =
                Func::lookup(&name).ok_or_else(|| format!("unknown function '{name}'"))?;
            let args = self.parse_arguments()?;
            if args.len() != arity {
                return Err(format!(
                    "'{name}' expects {arity} argument(s), got {}",
                    args.len()
                ));
            }
            return Ok(Node::Call(func, args));
        }
        match name.as_str() {
            "math.pi" => Ok(Node::Number(PI)),
            "true" => Ok(Node::Number(1.0)),
            "false" => Ok(Node::Number(0.0)),
            _ if name.starts_with("math.") => Err(format!(
                "'{name}' is not a constant; call it with arguments"
            )),
            _ => Ok(Node::Query(name.into_boxed_str())),
        }
    }

    fn parse_arguments(&mut self) -> Result<Vec<Node>, String> {
        let mut args = Vec::new();
        if self.eat(&Token::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_ternary()?);
            if !self.eat(&Token::Comma) {
                self.expect(&Token::RParen, "')'")?;
                return Ok(args);
            }
        }
    }
}

fn normalize_name(name: String) -> String {
    if let Some(rest) = name.strip_prefix("q.") {
        format!("query.{rest}")
    } else if let Some(rest) = name.strip_prefix("v.") {
        format!("variable.{rest}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Values(HashMap<&'static str, f32>);

    impl MolangContext for Values {
        fn query(&self, name: &str) -> f32 {
            self.0.get(name).copied().unwrap_or(0.0)
        }
    }

    fn values(pairs: &[(&'static str, f32)]) -> Values {
        Values(pairs.iter().copied().collect())
    }

    fn eval(src: &str) -> f32 {
        compile(src)
            .expect("expression should compile")
            .eval(&values(&[]))
    }

    #[test]
    fn parallel_entity_queries_are_deterministic_and_isolated() {
        let expression =
            compile("return math.sin(query.anim_time * 30 + variable.x) * query.ground_speed;")
                .unwrap();
        let inputs: Vec<_> = (0..129)
            .map(|i| (i as f32 * 0.125, i as f32 * 0.03125))
            .collect();
        let evaluate = |&(age, speed): &(f32, f32)| {
            expression
                .eval(&values(&[
                    ("query.anim_time", age),
                    ("query.ground_speed", speed),
                    ("variable.x", speed * 2.0),
                ]))
                .to_bits()
        };
        let serial: Vec<_> = inputs.iter().map(&evaluate).collect();
        assert!(serial.windows(2).any(|pair| pair[0] != pair[1]));
        for threads in [1, 2, 6] {
            assert_eq!(
                crate::par::par_map_with(threads, &inputs, &evaluate),
                serial
            );
        }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        assert_close(eval("1 + 2 * 3"), 7.0);
        assert_close(eval("(1 + 2) * 3"), 9.0);
    }

    #[test]
    fn subtraction_and_division_are_left_associative() {
        assert_close(eval("10 - 4 - 3"), 3.0);
        assert_close(eval("8 / 2 / 2"), 2.0);
    }

    #[test]
    fn unary_minus_and_not_bind_to_their_operand() {
        assert_close(eval("-2 * 3"), -6.0);
        assert_close(eval("2 * -3"), -6.0);
        assert_close(eval("!0 + 1"), 2.0);
    }

    #[test]
    fn comparison_binds_tighter_than_equality_and_logic() {
        assert_close(eval("1 + 2 < 4 == 1"), 1.0);
        assert_close(eval("1 || 0 && 0"), 1.0);
        assert_close(eval("3 >= 4"), 0.0);
        assert_close(eval("3 != 4"), 1.0);
    }

    #[test]
    fn math_sin_and_cos_take_degrees() {
        assert_close(eval("math.sin(90)"), 1.0);
        assert_close(eval("math.sin(30)"), 0.5);
        assert_close(eval("math.cos(60)"), 0.5);
        assert_close(eval("math.cos(0)"), 1.0);
    }

    #[test]
    fn ternary_selects_branch_and_nests_to_the_right() {
        assert_close(eval("1 ? 10 : 20"), 10.0);
        assert_close(eval("0 ? 10 : 20"), 20.0);
        assert_close(eval("0 ? 1 : 0 ? 2 : 3"), 3.0);
    }

    #[test]
    fn unknown_query_evaluates_to_zero() {
        assert_close(eval("query.missing + 3"), 3.0);
        assert_close(eval("NaN"), 0.0);
    }

    #[test]
    fn names_are_case_insensitive_and_aliases_are_normalized() {
        let ctx = values(&[("query.anim_time", 4.0), ("variable.x", 5.0)]);
        let expr = compile("Q.Anim_Time + V.X + MATH.SIN(90)").expect("compiles");
        assert_close(expr.eval(&ctx), 10.0);
    }

    #[test]
    fn return_statement_ends_evaluation() {
        assert_close(eval("1; return 5; 9"), 5.0);
        assert_close(eval("return 2 * 3;"), 6.0);
        assert_close(eval("1; 2"), 2.0);
    }

    #[test]
    fn division_by_zero_yields_zero() {
        assert_close(eval("5 / 0"), 0.0);
        assert_close(eval("math.mod(5, 0)"), 0.0);
    }

    #[test]
    fn math_helpers_match_bedrock_semantics() {
        assert_close(eval("math.clamp(15, 0, 10)"), 10.0);
        assert_close(eval("math.lerp(0, 10, 0.25)"), 2.5);
        assert_close(eval("math.pow(2, 3)"), 8.0);
        assert_close(eval("math.sqrt(16)"), 4.0);
        assert_close(eval("math.floor(-1.5)"), -2.0);
        assert_close(eval("math.ceil(1.2)"), 2.0);
        assert_close(eval("math.round(2.5)"), 3.0);
        assert_close(eval("math.min(3, 2)"), 2.0);
        assert_close(eval("math.max(3, 2)"), 3.0);
        assert_close(eval("math.mod(7, 4)"), 3.0);
        assert_close(eval("math.abs(-4)"), 4.0);
        assert_close(eval("math.pi"), PI);
    }

    #[test]
    fn query_values_feed_into_arithmetic() {
        let ctx = values(&[("query.life_time", 0.5)]);
        let expr = compile("-5 * math.cos(query.life_time * 360)").expect("compiles");
        assert_close(expr.eval(&ctx), 5.0);
    }

    #[test]
    fn empty_source_evaluates_to_zero() {
        assert_close(eval(""), 0.0);
    }

    #[test]
    fn invalid_sources_fail_to_compile() {
        assert!(compile("1 +").is_err());
        assert!(compile("(1").is_err());
        assert!(compile("1 = 2").is_err());
        assert!(compile("1 & 2").is_err());
        assert!(compile("@").is_err());
        assert!(compile("math.foo(1)").is_err());
        assert!(compile("math.sin(1, 2)").is_err());
        assert!(compile("math.sin").is_err());
        assert!(compile("1 2").is_err());
    }
}
