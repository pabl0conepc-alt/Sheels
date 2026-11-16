//! Calculator for the launcher's `>calc`. Uses `qalc` (libqalculate, like
//! Caelestia) when installed, otherwise a small built-in evaluator.

pub async fn eval(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return None;
    }
    if crate::util::which("qalc")
        && let Some(out) = crate::util::run(&["qalc", "-t", "-set", "autocalc off", expr], 2000).await {
            let r = out.trim().to_string();
            if !r.is_empty() && !r.to_lowercase().contains("error") {
                return Some(r);
            }
        }
    let v = Parser { s: expr.as_bytes(), i: 0 }.parse()?;
    if !v.is_finite() {
        return None;
    }
    let r = if (v - v.round()).abs() < 1e-10 && v.abs() < 1e15 { format!("{}", v.round() as i64) } else { format!("{:.10}", v) };
    let r = if r.contains('.') { r.trim_end_matches('0').trim_end_matches('.').to_string() } else { r };
    Some(r)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn parse(mut self) -> Option<f64> {
        let v = self.expr()?;
        self.ws();
        if self.i == self.s.len() { Some(v) } else { None }
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        while let Some(c) = self.peek() {
            match c {
                b'+' => {
                    self.i += 1;
                    v += self.term()?;
                }
                b'-' => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => break,
            }
        }
        Some(v)
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            match self.peek() {
                Some(b'*' | b'x') => {
                    self.i += 1;
                    v *= self.power()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    v /= self.power()?;
                }
                Some(b'%') => {
                    self.i += 1;
                    v %= self.power()?;
                }
                Some(b'(') => v *= self.power()?, // implicit multiplication: 2(3)
                _ => break,
            }
        }
        Some(v)
    }
    fn power(&mut self) -> Option<f64> {
        let b = self.unary()?;
        if self.peek() == Some(b'^') {
            self.i += 1;
            let e = self.power()?; // right associative
            return Some(b.powf(e));
        }
        Some(b)
    }
    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Some(-self.unary()?)
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            _ => self.atom(),
        }
    }
    fn atom(&mut self) -> Option<f64> {
        let c = self.peek()?;
        if c == b'(' {
            self.i += 1;
            let v = self.expr()?;
            if self.peek() != Some(b')') {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        if c.is_ascii_digit() || c == b'.' {
            let st = self.i;
            while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                self.i += 1;
            }
            if self.i < self.s.len() && (self.s[self.i] == b'e' || self.s[self.i] == b'E') {
                let save = self.i;
                self.i += 1;
                if self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i] == b'+') {
                    self.i += 1;
                }
                if self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                        self.i += 1;
                    }
                } else {
                    self.i = save;
                }
            }
            let mut v: f64 = std::str::from_utf8(&self.s[st..self.i]).ok()?.parse().ok()?;
            if self.peek() == Some(b'!') {
                self.i += 1;
                v = (1..=(v as u64).min(170)).map(|x| x as f64).product();
            }
            return Some(v);
        }
        if c.is_ascii_alphabetic() {
            let st = self.i;
            while self.i < self.s.len() && self.s[self.i].is_ascii_alphanumeric() {
                self.i += 1;
            }
            let name = std::str::from_utf8(&self.s[st..self.i]).ok()?.to_lowercase();
            match name.as_str() {
                "pi" => return Some(std::f64::consts::PI),
                "e" => return Some(std::f64::consts::E),
                "tau" => return Some(std::f64::consts::TAU),
                _ => {}
            }
            let arg = self.atom()?;
            return Some(match name.as_str() {
                "sqrt" => arg.sqrt(),
                "cbrt" => arg.cbrt(),
                "sin" => arg.sin(),
                "cos" => arg.cos(),
                "tan" => arg.tan(),
                "asin" => arg.asin(),
                "acos" => arg.acos(),
                "atan" => arg.atan(),
                "ln" => arg.ln(),
                "log" => arg.log10(),
                "log2" => arg.log2(),
                "abs" => arg.abs(),
                "exp" => arg.exp(),
                "floor" => arg.floor(),
                "ceil" => arg.ceil(),
                "round" => arg.round(),
                _ => return None,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    fn ev(s: &str) -> Option<f64> {
        super::Parser { s: s.as_bytes(), i: 0 }.parse()
    }
    #[test]
    fn arithmetic() {
        assert_eq!(ev("2+3*4"), Some(14.0));
        assert_eq!(ev("(2+3)*4"), Some(20.0));
        assert_eq!(ev("2^3^2"), Some(512.0));
        assert_eq!(ev("-3+5"), Some(2.0));
        assert_eq!(ev("10%3"), Some(1.0));
        assert_eq!(ev("5!"), Some(120.0));
        assert_eq!(ev("2(3)"), Some(6.0));
        assert_eq!(ev("1.5e3"), Some(1500.0));
        assert!((ev("sqrt(16)+sin(0)").unwrap() - 4.0).abs() < 1e-9);
        assert!((ev("pi").unwrap() - std::f64::consts::PI).abs() < 1e-12);
    }
    #[test]
    fn rejects_garbage() {
        assert_eq!(ev("2+"), None);
        assert_eq!(ev("foo(2)"), None);
        assert_eq!(ev("(1"), None);
        assert_eq!(ev("1)"), None);
        assert_eq!(ev(""), None);
    }
}
