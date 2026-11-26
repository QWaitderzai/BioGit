//! Примитивный вычислитель формул для встроенного табличного режима.
//!
//! Используется, когда Excel на машине отсутствует (см. [`crate::excel`]).
//! Это максимально простая реализация — ни в какое сравнение с движком Excel
//! она не претендует. Поддерживается:
//! - числа, строки в кавычках, логические TRUE/FALSE;
//! - ссылки на ячейки `A1`, `$A$1` и диапазоны `A1:B2`;
//! - арифметика `+ - * / ^`, скобки, унарный минус;
//! - сравнения `= <> < > <= >=`;
//! - функции `SUM`, `AVERAGE`, `MIN`, `MAX`, `COUNT`, `ROUND`, `ABS`, `IF`.
//!
//! Всё остальное даёт ошибку `#ОШИБКА!`. Формулы хранятся с ведущим `=`.

use std::collections::HashMap;

use crate::excel::SheetGrid;

/// Максимальное число ячеек в диапазоне одной формулы.
const MAX_RANGE_CELLS: usize = 1_000_000;

/// Значение вычисления формулы.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Str(String),
    Bool(bool),
    Err(String),
}

impl Value {
    /// Отображаемая строка значения.
    pub fn display(&self) -> String {
        match self {
            Value::Num(n) => format_number(*n),
            Value::Str(s) => s.clone(),
            Value::Bool(b) => {
                if *b {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }
            }
            Value::Err(e) => e.clone(),
        }
    }

    fn to_number(&self) -> Result<f64, String> {
        match self {
            Value::Num(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Str(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Ok(0.0);
                }
                trimmed
                    .replace(',', ".")
                    .parse::<f64>()
                    .map_err(|_| "#ЗНАЧ!".to_string())
            }
            Value::Err(e) => Err(e.clone()),
        }
    }
}

/// Число без «хвостов» плавающей точки: 5 → "5", 0.3+0.1 → "0.4".
pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "#ЧИСЛО!".to_string();
    }
    if n.is_infinite() {
        return "#ЧИСЛО!".to_string();
    }
    if n == 0.0 {
        return "0".to_string();
    }
    let rounded = (n * 1e10).round() / 1e10;
    if rounded == rounded.trunc() && rounded.abs() < 1e15 {
        format!("{}", rounded.trunc() as i64)
    } else {
        let s = format!("{rounded}");
        s
    }
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    Str(String),
    Bool(bool),
    /// Ссылка на ячейку (0-индексация).
    Ref(usize, usize),
    /// Диапазон ячеек (0-индексация, включительно).
    Range(usize, usize, usize, usize),
    Func(String, Vec<Node>),
    Neg(Box<Node>),
    Bin(String, Box<Node>, Box<Node>),
}

// ---------------------------------------------------------------------------
// Токенизация
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Str(String),
    Ident(String),
    Ref(String), // «A1» или «$A$1» — разбирается в узел позже
    Op(String),
    LParen,
    RParen,
    Comma,
    Colon,
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '(' => {
                out.push(Token::LParen);
                chars.next();
            }
            ')' => {
                out.push(Token::RParen);
                chars.next();
            }
            ',' | ';' => {
                out.push(Token::Comma);
                chars.next();
            }
            ':' => {
                out.push(Token::Colon);
                chars.next();
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                while let Some(ch) = chars.next() {
                    if ch == '"' {
                        // удвоенная кавычка внутри строки
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            s.push('"');
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        s.push(ch);
                    }
                }
                if !closed {
                    return Err("#ОШИБКА!".to_string());
                }
                out.push(Token::Str(s));
            }
            '+' | '-' | '*' | '/' | '^' | '=' | '<' | '>' => {
                let mut op = String::new();
                op.push(c);
                chars.next();
                if (c == '<' || c == '>')
                    && matches!(chars.peek(), Some('=') | Some('>'))
                {
                    op.push(chars.next().unwrap());
                }
                out.push(Token::Op(op));
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let mut s = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_digit() || ch == '.' {
                        s.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n = s.parse::<f64>().map_err(|_| "#ОШИБКА!".to_string())?;
                out.push(Token::Num(n));
            }
            _ if c.is_alphabetic() || c == '$' => {
                let mut s = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '$' || ch == '_' {
                        s.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // «A1» — ссылка; «SUM»/«СУММ» — имя функции
                let is_ref = {
                    let letters = s.trim_start_matches('$');
                    let split = letters.find(|ch: char| ch.is_ascii_digit());
                    match split {
                        Some(pos) if pos > 0 => letters[..pos].chars().all(|ch| ch.is_ascii_uppercase())
                            && letters[pos..].chars().all(|ch| ch.is_ascii_digit()),
                        _ => false,
                    }
                };
                if is_ref {
                    out.push(Token::Ref(s));
                } else {
                    out.push(Token::Ident(s.to_ascii_uppercase()));
                }
            }
            _ => return Err("#ОШИБКА!".to_string()),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Парсер (рекурсивный спуск)
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Result<Node, String> {
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> Result<Node, String> {
        let mut left = self.parse_add()?;
        while let Some(Token::Op(op)) = self.peek() {
            let op = op.clone();
            if matches!(op.as_str(), "=" | "<>" | "<" | ">" | "<=" | ">=") {
                self.next();
                let right = self.parse_add()?;
                left = Node::Bin(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Node, String> {
        let mut left = self.parse_mul()?;
        while let Some(Token::Op(op)) = self.peek() {
            let op = op.clone();
            if matches!(op.as_str(), "+" | "-") {
                self.next();
                let right = self.parse_mul()?;
                left = Node::Bin(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Node, String> {
        let mut left = self.parse_unary()?;
        while let Some(Token::Op(op)) = self.peek() {
            let op = op.clone();
            if matches!(op.as_str(), "*" | "/") {
                self.next();
                let right = self.parse_unary()?;
                left = Node::Bin(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Node, String> {
        if let Some(Token::Op(op)) = self.peek() {
            let op = op.clone();
            if op == "-" {
                self.next();
                let inner = self.parse_unary()?;
                return Ok(Node::Neg(Box::new(inner)));
            }
            if op == "+" {
                self.next();
                return self.parse_unary();
            }
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<Node, String> {
        let base = self.parse_atom()?;
        if let Some(Token::Op(op)) = self.peek() {
            if op == "^" {
                self.next();
                let exp = self.parse_unary()?;
                return Ok(Node::Bin("^".to_string(), Box::new(base), Box::new(exp)));
            }
        }
        Ok(base)
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        match self.next() {
            Some(Token::Num(n)) => Ok(Node::Num(n)),
            Some(Token::Str(s)) => Ok(Node::Str(s)),
            Some(Token::Ref(r)) => {
                // возможен диапазон A1:B2
                if self.peek() == Some(&Token::Colon) {
                    self.next();
                    match self.next() {
                        Some(Token::Ref(r2)) => {
                            let (r1, c1) = parse_ref(&r).ok_or_else(|| "#ОШИБКА!".to_string())?;
                            let (r2, c2) = parse_ref(&r2).ok_or_else(|| "#ОШИБКА!".to_string())?;
                            Ok(Node::Range(r1, c1, r2, c2))
                        }
                        _ => Err("#ОШИБКА!".to_string()),
                    }
                } else {
                    let (row, col) = parse_ref(&r).ok_or_else(|| "#ОШИБКА!".to_string())?;
                    Ok(Node::Ref(row, col))
                }
            }
            Some(Token::Ident(name)) => {
                // TRUE/FALSE без скобок
                if matches!(name.as_str(), "TRUE" | "FALSE") {
                    return Ok(Node::Bool(name == "TRUE"));
                }
                match self.next() {
                    Some(Token::LParen) => {
                        let mut args = Vec::new();
                        if self.peek() != Some(&Token::RParen) {
                            loop {
                                args.push(self.parse_expr()?);
                                match self.next() {
                                    Some(Token::Comma) => continue,
                                    Some(Token::RParen) => break,
                                    _ => return Err("#ОШИБКА!".to_string()),
                                }
                            }
                        } else {
                            self.next(); // RParen
                        }
                        Ok(Node::Func(name, args))
                    }
                    _ => Err("#ОШИБКА!".to_string()),
                }
            }
            Some(Token::LParen) => {
                let inner = self.parse_expr()?;
                match self.next() {
                    Some(Token::RParen) => Ok(inner),
                    _ => Err("#ОШИБКА!".to_string()),
                }
            }
            _ => Err("#ОШИБКА!".to_string()),
        }
    }
}

/// Разбирает ссылку вида `A1` / `$A$1` в (строка, колонка), 0-индексация.
pub fn parse_ref(text: &str) -> Option<(usize, usize)> {
    let cleaned: String = text.chars().filter(|c| *c != '$').collect();
    let split = cleaned.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = cleaned.split_at(split);
    if letters.is_empty() || digits.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut col = 0usize;
    for c in letters.chars() {
        let upper = c.to_ascii_uppercase();
        if !upper.is_ascii_uppercase() {
            return None;
        }
        col = col * 26 + (upper as usize - 'A' as usize + 1);
    }
    let row: usize = digits.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

// ---------------------------------------------------------------------------
// Вычисление
// ---------------------------------------------------------------------------

/// Оценивает узел; диапазон возвращает как список чисел (для функций).
fn eval_node(
    node: &Node,
    grid: &SheetGrid,
    memo: &mut HashMap<(usize, usize), Value>,
    visiting: &mut Vec<(usize, usize)>,
) -> Value {
    match node {
        Node::Num(n) => Value::Num(*n),
        Node::Str(s) => Value::Str(s.clone()),
        Node::Bool(b) => Value::Bool(*b),
        Node::Ref(r, c) => cell_value(grid, *r, *c, memo, visiting),
        Node::Range(..) => {
            let nums = range_numbers(node, grid, memo, visiting);
            match nums {
                Ok(nums) => Value::Num(nums.iter().sum()),
                Err(e) => Value::Err(e),
            }
        }
        Node::Neg(inner) => match eval_node(inner, grid, memo, visiting) {
            Value::Err(e) => Value::Err(e),
            other => match other.to_number() {
                Ok(n) => Value::Num(-n),
                Err(e) => Value::Err(e),
            },
        },
        Node::Bin(op, left, right) => {
            let lv = eval_node(left, grid, memo, visiting);
            let rv = eval_node(right, grid, memo, visiting);
            bin_op(op, lv, rv)
        }
        Node::Func(name, args) => eval_func(name, args, grid, memo, visiting),
    }
}

fn bin_op(op: &str, lv: Value, rv: Value) -> Value {
    if let Value::Err(e) = lv {
        return Value::Err(e);
    }
    if let Value::Err(e) = rv {
        return Value::Err(e);
    }
    match op {
        "=" | "<>" | "<" | ">" | "<=" | ">=" => {
            let cmp = compare_values(&lv, &rv);
            match cmp {
                Err(e) => Value::Err(e),
                Ok(ord) => {
                    let result = match op {
                        "=" => ord == std::cmp::Ordering::Equal,
                        "<>" => ord != std::cmp::Ordering::Equal,
                        "<" => ord == std::cmp::Ordering::Less,
                        ">" => ord == std::cmp::Ordering::Greater,
                        "<=" => ord != std::cmp::Ordering::Greater,
                        ">=" => ord != std::cmp::Ordering::Less,
                        _ => false,
                    };
                    Value::Bool(result)
                }
            }
        }
        "+" | "-" | "*" | "/" | "^" => {
            let l = match lv.to_number() {
                Ok(n) => n,
                Err(e) => return Value::Err(e),
            };
            let r = match rv.to_number() {
                Ok(n) => n,
                Err(e) => return Value::Err(e),
            };
            let result = match op {
                "+" => l + r,
                "-" => l - r,
                "*" => l * r,
                "/" => {
                    if r == 0.0 {
                        return Value::Err("#ДЕЛ/0!".to_string());
                    }
                    l / r
                }
                "^" => l.powf(r),
                _ => return Value::Err("#ОШИБКА!".to_string()),
            };
            if result.is_nan() || result.is_infinite() {
                return Value::Err("#ЧИСЛО!".to_string());
            }
            Value::Num(result)
        }
        _ => Value::Err("#ОШИБКА!".to_string()),
    }
}

fn compare_values(l: &Value, r: &Value) -> Result<std::cmp::Ordering, String> {
    use std::cmp::Ordering;
    match (l, r) {
        (Value::Str(a), Value::Str(b)) => Ok(a.to_lowercase().cmp(&b.to_lowercase())),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        _ => {
            let a = l.to_number()?;
            let b = r.to_number()?;
            Ok(a.partial_cmp(&b).unwrap_or(Ordering::Equal))
        }
    }
}

/// Собирает числа из диапазона/аргумента функции.
fn range_numbers(
    node: &Node,
    grid: &SheetGrid,
    memo: &mut HashMap<(usize, usize), Value>,
    visiting: &mut Vec<(usize, usize)>,
) -> Result<Vec<f64>, String> {
    match node {
        Node::Range(r1, c1, r2, c2) => {
            let (r1, r2) = (*r1.min(r2), *r1.max(r2));
            let (c1, c2) = (*c1.min(c2), *c1.max(c2));
            if (r2 - r1 + 1).saturating_mul(c2 - c1 + 1) > MAX_RANGE_CELLS {
                return Err("#ОШИБКА!".to_string());
            }
            let mut out = Vec::new();
            for r in r1..=r2 {
                for c in c1..=c2 {
                    if let Value::Num(n) = cell_value(grid, r, c, memo, visiting) {
                        out.push(n);
                    }
                }
            }
            Ok(out)
        }
        Node::Num(n) => Ok(vec![*n]),
        other => {
            let v = eval_node(other, grid, memo, visiting);
            match v {
                Value::Num(n) => Ok(vec![n]),
                Value::Err(e) => Err(e),
                _ => Ok(Vec::new()), // текст в агрегатах игнорируется
            }
        }
    }
}

fn eval_func(
    name: &str,
    args: &[Node],
    grid: &SheetGrid,
    memo: &mut HashMap<(usize, usize), Value>,
    visiting: &mut Vec<(usize, usize)>,
) -> Value {
    let err = |msg: &str| Value::Err(msg.to_string());
    match name {
        "SUM" => {
            let mut sum = 0.0;
            for a in args {
                match range_numbers(a, grid, memo, visiting) {
                    Ok(nums) => sum += nums.iter().sum::<f64>(),
                    Err(e) => return err(&e),
                }
            }
            Value::Num(sum)
        }
        "AVERAGE" | "СРЗНАЧ" => {
            let mut all = Vec::new();
            for a in args {
                match range_numbers(a, grid, memo, visiting) {
                    Ok(nums) => all.extend(nums),
                    Err(e) => return err(&e),
                }
            }
            if all.is_empty() {
                return err("#ДЕЛ/0!");
            }
            Value::Num(all.iter().sum::<f64>() / all.len() as f64)
        }
        "MIN" | "МИН" => {
            let mut all = Vec::new();
            for a in args {
                match range_numbers(a, grid, memo, visiting) {
                    Ok(nums) => all.extend(nums),
                    Err(e) => return err(&e),
                }
            }
            Value::Num(all.iter().copied().fold(f64::INFINITY, f64::min))
        }
        "MAX" | "МАКС" => {
            let mut all = Vec::new();
            for a in args {
                match range_numbers(a, grid, memo, visiting) {
                    Ok(nums) => all.extend(nums),
                    Err(e) => return err(&e),
                }
            }
            Value::Num(all.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        }
        "COUNT" | "СЧЁТ" => {
            let mut count = 0usize;
            for a in args {
                match range_numbers(a, grid, memo, visiting) {
                    Ok(nums) => count += nums.len(),
                    Err(e) => return err(&e),
                }
            }
            Value::Num(count as f64)
        }
        "ABS" => match args.len() {
            1 => match eval_node(&args[0], grid, memo, visiting).to_number() {
                Ok(n) => Value::Num(n.abs()),
                Err(e) => err(&e),
            },
            _ => err("#ОШИБКА!"),
        },
        "ROUND" | "ОКРУГЛ" => {
            if args.len() != 2 {
                return err("#ОШИБКА!");
            }
            let n = match eval_node(&args[0], grid, memo, visiting).to_number() {
                Ok(n) => n,
                Err(e) => return err(&e),
            };
            let digits = match eval_node(&args[1], grid, memo, visiting).to_number() {
                Ok(d) => d,
                Err(e) => return err(&e),
            };
            let factor = 10f64.powf(digits);
            Value::Num((n * factor).round() / factor)
        }
        "IF" | "ЕСЛИ" => {
            if args.is_empty() {
                return err("#ОШИБКА!");
            }
            let cond = eval_node(&args[0], grid, memo, visiting);
            let truthy = match cond {
                Value::Bool(b) => b,
                Value::Num(n) => n != 0.0,
                Value::Err(e) => return err(&e),
                Value::Str(s) => !s.is_empty(),
            };
            if truthy {
                if args.len() > 1 {
                    eval_node(&args[1], grid, memo, visiting)
                } else {
                    Value::Bool(true)
                }
            } else if args.len() > 2 {
                eval_node(&args[2], grid, memo, visiting)
            } else {
                Value::Bool(false)
            }
        }
        other => err(&format!("#ИМЯ? ({other})")),
    }
}

/// Читает значение ячейки (с вычислением формул по цепочке).
fn cell_value(
    grid: &SheetGrid,
    row: usize,
    col: usize,
    memo: &mut HashMap<(usize, usize), Value>,
    visiting: &mut Vec<(usize, usize)>,
) -> Value {
    let cell = match grid.rows.get(row).and_then(|r| r.get(col)) {
        Some(cell) => cell,
        None => return Value::Num(0.0), // за границами сетки — пусто (как 0)
    };
    match &cell.formula {
        Some(formula) => {
            let key = (row, col);
            if let Some(v) = memo.get(&key) {
                return v.clone();
            }
            if visiting.contains(&key) {
                return Value::Err("#ЦИКЛ!".to_string());
            }
            visiting.push(key);
            let v = eval_formula_text(formula, grid, memo, visiting);
            visiting.pop();
            memo.insert(key, v.clone());
            v
        }
        None => {
            let raw = &cell.value;
            if raw.is_empty() {
                return Value::Num(0.0);
            }
            if raw == "TRUE" {
                return Value::Bool(true);
            }
            if raw == "FALSE" {
                return Value::Bool(false);
            }
            if let Ok(n) = raw.replace(',', ".").parse::<f64>() {
                return Value::Num(n);
            }
            Value::Str(raw.clone())
        }
    }
}

fn eval_formula_text(
    formula: &str,
    grid: &SheetGrid,
    memo: &mut HashMap<(usize, usize), Value>,
    visiting: &mut Vec<(usize, usize)>,
) -> Value {
    let text = formula.strip_prefix('=').unwrap_or(formula);
    let tokens = match tokenize(text) {
        Ok(t) => t,
        Err(e) => return Value::Err(e),
    };
    if tokens.is_empty() {
        return Value::Str(String::new());
    }
    let mut parser = Parser { tokens, pos: 0 };
    let node = match parser.parse_expr() {
        Ok(n) => n,
        Err(e) => return Value::Err(e),
    };
    if parser.pos != parser.tokens.len() {
        return Value::Err("#ОШИБКА!".to_string());
    }
    eval_node(&node, grid, memo, visiting)
}

/// Пересчитывает все формулы сетки и обновляет отображаемые значения.
/// Возвращает число пересчитанных формул.
pub fn evaluate_sheet(grid: &mut SheetGrid) -> usize {
    let positions: Vec<(usize, usize)> = grid
        .rows
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.iter()
                .enumerate()
                .filter(|(_, c)| c.formula.is_some())
                .map(move |(c, _)| (r, c))
        })
        .collect();

    let mut memo: HashMap<(usize, usize), Value> = HashMap::new();
    let mut visiting: Vec<(usize, usize)> = Vec::new();
    let mut count = 0usize;
    for (r, c) in positions {
        let v = cell_value(grid, r, c, &mut memo, &mut visiting);
        if let Some(cell) = grid.rows.get_mut(r).and_then(|row| row.get_mut(c)) {
            cell.value = v.display();
        }
        count += 1;
    }
    count
}

/// Вычисляет одну формулу вне сетки (без ссылок; для диагностики).
pub fn eval_standalone(text: &str) -> String {
    let mut memo = HashMap::new();
    let mut visiting = Vec::new();
    let grid = SheetGrid {
        name: String::new(),
        rows: Vec::new(),
    };
    eval_formula_text(text, &grid, &mut memo, &mut visiting).display()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel::SheetCell;

    fn grid_from(values: &[&[&str]], formulas: &[&[Option<&str>]]) -> SheetGrid {
        let mut rows = Vec::new();
        for (r, row_vals) in values.iter().enumerate() {
            let mut row = Vec::new();
            for (c, v) in row_vals.iter().enumerate() {
                let formula = formulas
                    .get(r)
                    .and_then(|f| f.get(c))
                    .and_then(|f| f.map(|s| s.to_string()));
                row.push(SheetCell {
                    value: v.to_string(),
                    formula,
                });
            }
            rows.push(row);
        }
        SheetGrid {
            name: "test".to_string(),
            rows,
        }
    }

    #[test]
    fn арифметика_и_приоритеты() {
        assert_eq!(eval_standalone("=1+2*3"), "7");
        assert_eq!(eval_standalone("=(1+2)*3"), "9");
        assert_eq!(eval_standalone("=2^10"), "1024");
        assert_eq!(eval_standalone("=-5+2"), "-3");
        assert_eq!(eval_standalone("=10/4"), "2.5");
        assert_eq!(eval_standalone("=1/0"), "#ДЕЛ/0!");
    }

    #[test]
    fn ссылки_и_диапазоны() {
        let mut grid = grid_from(
            &[&["10", "20", "30"], &["=A1+B1", "=SUM(A1:C1)"]],
            &[&[None, None, None], &[Some("=A1+B1"), Some("=SUM(A1:C1)"), None]],
        );
        // вторая строка короче — дополняем
        while grid.rows[1].len() < 3 {
            grid.rows[1].push(SheetCell {
                value: String::new(),
                formula: None,
            });
        }
        evaluate_sheet(&mut grid);
        assert_eq!(grid.rows[1][0].value, "30");
        assert_eq!(grid.rows[1][1].value, "60");
    }

    #[test]
    fn функции_агрегаты() {
        let mut grid = grid_from(
            &[&["1", "2", "3", "=SUM(A1:C1)", "=AVERAGE(A1:C1)", "=MAX(A1:C1)", "=MIN(A1:C1)"]],
            &[&[
                None,
                None,
                None,
                Some("=SUM(A1:C1)"),
                Some("=AVERAGE(A1:C1)"),
                Some("=MAX(A1:C1)"),
                Some("=MIN(A1:C1)"),
            ]],
        );
        evaluate_sheet(&mut grid);
        assert_eq!(grid.rows[0][3].value, "6");
        assert_eq!(grid.rows[0][4].value, "2");
        assert_eq!(grid.rows[0][5].value, "3");
        assert_eq!(grid.rows[0][6].value, "1");
    }

    #[test]
    fn если_и_сравнения() {
        assert_eq!(eval_standalone("=IF(1>0, 10, 20)"), "10");
        assert_eq!(eval_standalone("=IF(1<0, 10, 20)"), "20");
        assert_eq!(eval_standalone("=2=2"), "TRUE");
        assert_eq!(eval_standalone("=2<>2"), "FALSE");
        assert_eq!(eval_standalone("=ROUND(3.14159, 2)"), "3.14");
        assert_eq!(eval_standalone("=ABS(-7)"), "7");
    }

    #[test]
    fn цепочка_формул_и_циклы() {
        // A1 = 1; B1 = A1*2; C1 = B1+1 → 3
        let mut grid = grid_from(
            &[&["1", "", ""], &[], &[]],
            &[&[None, Some("=A1*2"), Some("=B1+1")]],
        );
        grid.rows[0].push(SheetCell {
            value: String::new(),
            formula: None,
        });
        grid.rows[0].pop();
        grid.rows[0].push(SheetCell {
            value: String::new(),
            formula: None,
        });
        evaluate_sheet(&mut grid);
        assert_eq!(grid.rows[0][1].value, "2");
        assert_eq!(grid.rows[0][2].value, "3");

        // цикл: D1 = D1+1
        let mut cyc = grid_from(
            &[&["1", ""]],
            &[&[None, Some("=D1+1")]],
        );
        cyc.rows[0][1].formula = Some("=A1+B1".to_string());
        evaluate_sheet(&mut cyc);
        assert_eq!(cyc.rows[0][1].value, "#ЦИКЛ!");
    }

    #[test]
    fn неизвестная_функция_даёт_ошибку() {
        assert!(eval_standalone("=ВПР(A1, B1, 2)").starts_with("#ИМЯ?"));
    }

    #[test]
    fn разбор_ссылок() {
        assert_eq!(parse_ref("A1"), Some((0, 0)));
        assert_eq!(parse_ref("$B$2"), Some((1, 1)));
        assert_eq!(parse_ref("AA10"), Some((9, 26)));
        assert_eq!(parse_ref("A0"), None);
        assert_eq!(parse_ref("1A"), None);
    }

    #[test]
    fn формат_чисел_без_хвостов() {
        assert_eq!(format_number(5.0), "5");
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(-2.5), "-2.5");
    }
}
