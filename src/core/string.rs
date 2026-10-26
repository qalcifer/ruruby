//! String. mruby: `src/string.c`, `mrblib/string.rb`, `mruby-string-ext`.
//!
//! Strings are Rust `String`s (UTF-8) and indexed by character, matching
//! mruby built with `MRB_UTF8_STRING`.

use super::*;
use crate::vm::{VmResult, inspect_str};

fn s_of(vm: &Vm, s: &Value) -> Result<String, Unwind> {
    Ok(vm.heap[this_str(vm, s)?].borrow().clone())
}

fn map_str(vm: &mut Vm, s: &Value, f: impl FnOnce(&str) -> String) -> VmResult {
    let r = f(&s_of(vm, s)?);
    Ok(vm.str(r))
}

/// An Array of new Strings.
fn str_array(vm: &mut Vm, parts: impl IntoIterator<Item = String>) -> Value {
    let v = parts.into_iter().map(|p| vm.str(p)).collect();
    vm.array(v)
}

/// Resolves a (possibly negative) index against `len`.
pub fn norm_index(i: i64, len: usize) -> Option<usize> {
    let i = if i < 0 { i + len as i64 } else { i };
    (0..=len as i64).contains(&i).then_some(i as usize)
}

pub fn init(vm: &mut Vm) {
    init_mutators(vm);
    let c = vm.c.string;
    // String.new(str), and a subclass's `super` in initialize
    vm.def(&c, "init", |vm, s, a, _| {
        vm.check_frozen(s)?;
        check_argc(vm, a, 0, 1)?;
        if let [x] = a {
            let text = str_of(vm, x)?;
            *vm.heap[this_str(vm, s)?].borrow_mut() = text;
        }
        Ok(*s)
    });
    vm.def(&c, "to_s", |_, s, _, _| Ok(*s));
    vm.def(&c, "to_str", |_, s, _, _| Ok(*s));
    vm.def(&c, "inspect", |vm, s, _, _| {
        Ok(vm.str(inspect_str(&s_of(vm, s)?)))
    });
    vm.def(&c, "dump", |vm, s, _, _| {
        Ok(vm.str(inspect_str(&s_of(vm, s)?)))
    });
    vm.def(&c, "to_sym", to_sym);
    vm.def(&c, "intern", to_sym);
    // to_i(base = 10): the leading digits after whitespace and a sign; one
    // `_` may separate digits
    vm.def(&c, "to_i", |vm, s, a, _| {
        let base = match a.first() {
            Some(b) => int_of(vm, b)?,
            None => 10,
        };
        if !(2..=36).contains(&base) {
            return Err(vm.arg_err(format!("invalid radix {base}")));
        }
        let text = s_of(vm, s)?;
        let mut chars = text.trim_start().chars().peekable();
        let neg = match chars.peek() {
            Some('-') => { chars.next(); true }
            Some('+') => { chars.next(); false }
            _ => false,
        };
        let mut n: i64 = 0;
        let mut last_digit = false;
        while let Some(&ch) = chars.peek() {
            if let Some(d) = ch.to_digit(base as u32) {
                n = n.wrapping_mul(base).wrapping_add(d as i64);
                last_digit = true;
            } else if ch == '_' && last_digit {
                chars.next();
                if !chars.peek().is_some_and(|c| c.is_digit(base as u32)) {
                    break;
                }
                last_digit = false;
                continue;
            } else {
                break;
            }
            chars.next();
        }
        Ok(vm.int(if neg { -n } else { n }))
    });
    vm.def(&c, "to_f", |vm, s, _, _| {
        let s = s_of(vm, s)?;
        let t = s.trim();
        let best = (0..=t.len())
            .rev()
            .filter(|&i| t.is_char_boundary(i))
            .find_map(|i| t[..i].parse::<f64>().ok());
        Ok(Value::float(best.unwrap_or(0.0)))
    });
    vm.def(&c, "length", |vm, s, _, _| {
        Ok(Value::small_int(s_of(vm, s)?.chars().count() as i64))
    });
    vm.def(&c, "size", |vm, s, _, _| {
        Ok(Value::small_int(s_of(vm, s)?.chars().count() as i64))
    });
    vm.def(&c, "bytesize", |vm, s, _, _| {
        Ok(Value::small_int(s_of(vm, s)?.len() as i64))
    });
    vm.def(&c, "empty?", |vm, s, _, _| {
        Ok(Value::bool(s_of(vm, s)?.is_empty()))
    });
    vm.def(&c, "upcase", |vm, s, _, _| {
        map_str(vm, s, |x| x.to_uppercase())
    });
    vm.def(&c, "downcase", |vm, s, _, _| {
        map_str(vm, s, |x| x.to_lowercase())
    });
    vm.def(&c, "swapcase", |vm, s, _, _| {
        map_str(vm, s, |x| {
            x.chars()
                .map(|c| {
                    if c.is_uppercase() {
                        c.to_lowercase().collect::<String>()
                    } else {
                        c.to_uppercase().collect()
                    }
                })
                .collect()
        })
    });
    vm.def(&c, "capitalize", |vm, s, _, _| {
        map_str(vm, s, |x| {
            let mut cs = x.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase(),
                None => String::new(),
            }
        })
    });
    vm.def(&c, "reverse", |vm, s, _, _| {
        map_str(vm, s, |x| x.chars().rev().collect())
    });
    vm.def(&c, "strip", |vm, s, _, _| {
        map_str(vm, s, |x| x.trim().to_string())
    });
    vm.def(&c, "lstrip", |vm, s, _, _| {
        map_str(vm, s, |x| x.trim_start().to_string())
    });
    vm.def(&c, "rstrip", |vm, s, _, _| {
        map_str(vm, s, |x| x.trim_end().to_string())
    });
    vm.def(&c, "chomp", |vm, s, _, _| {
        map_str(vm, s, |x| {
            x.strip_suffix("\r\n")
                .or_else(|| x.strip_suffix('\n'))
                .unwrap_or(x)
                .to_string()
        })
    });
    vm.def(&c, "chop", |vm, s, _, _| {
        map_str(vm, s, |x| {
            let mut v = x.to_string();
            v.pop();
            v
        })
    });
    vm.def(&c, "chars", |vm, s, _, _| {
        let chars: Vec<String> = s_of(vm, s)?.chars().map(String::from).collect();
        Ok(str_array(vm, chars))
    });
    vm.def(&c, "bytes", |vm, s, _, _| {
        Ok(vm.array(
            s_of(vm, s)?.bytes().map(|b| Value::small_int(b as i64)).collect(),
        ))
    });
    vm.def(&c, "lines", |vm, s, _, _| {
        let lines: Vec<String> = s_of(vm, s)?.split_inclusive('\n').map(String::from).collect();
        Ok(str_array(vm, lines))
    });
    vm.def(&c, "each_char", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for ch in s_of(vm, s)?.chars() {
            let ch = vm.str(ch.to_string());
            vm.call_block(&b, vec![ch])?;
        }
        Ok(*s)
    });
    vm.def(&c, "each_line", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for line in s_of(vm, s)?.split_inclusive('\n') {
            let line = vm.str(line);
            vm.call_block(&b, vec![line])?;
        }
        Ok(*s)
    });
    vm.def(&c, "ord", |vm, s, _, _| {
        s_of(vm, s)?
            .chars()
            .next()
            .map(|c| Value::small_int(c as i64))
            .ok_or_else(|| vm.arg_err("empty string"))
    });
    vm.def(&c, "+", |vm, s, a, _| {
        let rhs = str_of(vm, &arg(a, 0))?;
        Ok(vm.str(s_of(vm, s)? + &rhs))
    });
    vm.def(&c, "*", |vm, s, a, _| {
        let x = arg(a, 0);
        if !matches!(vm.kind(x), V::Int(_) | V::Float(_)) {
            return Err(vm.type_err(format!("no implicit conversion of {} into Integer", vm.class_name(&x))));
        }
        let n = int_of(vm, &x)?;
        if n < 0 {
            return Err(vm.arg_err("negative argument"));
        }
        let text = s_of(vm, s)?;
        if (text.len() as u128) * (n as u128) > (1u128 << 32) {
            return Err(vm.err(&vm.c.range_error, "argument too big"));
        }
        Ok(vm.str(text.repeat(n as usize)))
    });
    vm.def(&c, "%", |vm, s, a, _| {
        let fmt = s_of(vm, s)?;
        let args = match vm.kind(arg(a, 0)) {
            V::Array(v) => vm.heap[v].borrow().clone(),
            _ => vec![arg(a, 0)],
        };
        let out = kernel::format(vm, &fmt, &args)?;
        Ok(vm.str(out))
    });
    let append: NativeFn = |vm, s, a, _| {
        vm.check_frozen(s)?;
        let piece = vm.to_s(&arg(a, 0))?;
        vm.heap[this_str(vm, s)?].borrow_mut().push_str(&piece);
        Ok(*s)
    };
    vm.def(&c, "<<", append);
    vm.def(&c, "concat", append);
    vm.def(&c, "replace", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let new = str_of(vm, &arg(a, 0))?;
        *vm.heap[this_str(vm, s)?].borrow_mut() = new;
        Ok(*s)
    });
    vm.def(&c, "==", |vm, s, a, _| {
        Ok(Value::bool(values_eq(&vm.heap, s, &arg(a, 0))))
    });
    vm.def(&c, "eql?", |vm, s, a, _| {
        Ok(Value::bool(values_eq(&vm.heap, s, &arg(a, 0))))
    });
    vm.def(&c, "<=>", |vm, s, a, _| match vm.kind(arg(a, 0)) {
        V::Str(o) => Ok(Value::small_int(
            s_of(vm, s)?.as_str().cmp(vm.heap[o].borrow().as_str()) as i64
        )),
        _ => Ok(Value::NIL),
    });
    vm.def(&c, "<", |vm, s, a, _| {
        Ok(Value::bool(compare(vm, s, &arg(a, 0))?.is_lt()))
    });
    vm.def(&c, "<=", |vm, s, a, _| {
        Ok(Value::bool(compare(vm, s, &arg(a, 0))?.is_le()))
    });
    vm.def(&c, ">", |vm, s, a, _| {
        Ok(Value::bool(compare(vm, s, &arg(a, 0))?.is_gt()))
    });
    vm.def(&c, ">=", |vm, s, a, _| {
        Ok(Value::bool(compare(vm, s, &arg(a, 0))?.is_ge()))
    });
    vm.def(&c, "include?", |vm, s, a, _| {
        let needle = str_of(vm, &arg(a, 0))?;
        Ok(Value::bool(s_of(vm, s)?.contains(&needle)))
    });
    vm.def(&c, "start_with?", |vm, s, a, _| {
        let s = s_of(vm, s)?;
        for p in a {
            if s.starts_with(&str_of(vm, p)?) {
                return Ok(Value::TRUE);
            }
        }
        Ok(Value::FALSE)
    });
    vm.def(&c, "end_with?", |vm, s, a, _| {
        let s = s_of(vm, s)?;
        for p in a {
            if s.ends_with(&str_of(vm, p)?) {
                return Ok(Value::TRUE);
            }
        }
        Ok(Value::FALSE)
    });
    // index(sub, offset = 0): characters; a negative offset counts from the end
    vm.def(&c, "index", |vm, s, a, _| {
        check_argc(vm, a, 1, 2)?;
        let needle: Vec<char> = str_arg(vm, &a[0])?.chars().collect();
        let hay: Vec<char> = s_of(vm, s)?.chars().collect();
        let len = hay.len() as i64;
        let from = match a.get(1) {
            Some(o) => index_arg(vm, o)?,
            None => 0,
        };
        let from = if from < 0 { from + len } else { from };
        if from < 0 || from > len {
            return Ok(Value::NIL);
        }
        let last = len - needle.len() as i64;
        Ok((from..=last)
            .find(|&i| hay[i as usize..].starts_with(&needle))
            .map(Value::small_int)
            .unwrap_or(Value::NIL))
    });
    vm.def(&c, "count", |vm, s, a, _| {
        let set: Vec<char> = str_of(vm, &arg(a, 0))?.chars().collect();
        Ok(Value::small_int(
            s_of(vm, s)?.chars().filter(|c| set.contains(c)).count() as i64,
        ))
    });
    vm.def(&c, "delete", |vm, s, a, _| {
        let set: Vec<char> = str_of(vm, &arg(a, 0))?.chars().collect();
        Ok(vm.str(
            s_of(vm, s)?
                .chars()
                .filter(|c| !set.contains(c))
                .collect::<String>(),
        ))
    });
    vm.def(&c, "squeeze", |vm, s, _, _| {
        let mut out = String::new();
        for ch in s_of(vm, s)?.chars() {
            if !out.ends_with(ch) {
                out.push(ch);
            }
        }
        Ok(vm.str(out))
    });
    vm.def(&c, "sub", |vm, s, a, b| substitute(vm, s, a, b, false));
    vm.def(&c, "gsub", |vm, s, a, b| substitute(vm, s, a, b, true));
    vm.def(&c, "tr", |vm, s, a, _| {
        let from: Vec<char> = str_of(vm, &arg(a, 0))?.chars().collect();
        let to: Vec<char> = str_of(vm, &arg(a, 1))?.chars().collect();
        let out = s_of(vm, s)?
            .chars()
            .map(|c| match from.iter().position(|f| *f == c) {
                Some(i) => *to.get(i).or(to.last()).unwrap_or(&c),
                None => c,
            })
            .collect::<String>();
        Ok(vm.str(out))
    });
    vm.def(&c, "split", |vm, s, a, _| {
        let s = s_of(vm, s)?;
        let parts: Vec<String> = match vm.kind(arg(a, 0)) {
            V::Nil => s.split_whitespace().map(String::from).collect(),
            _ => {
                let sep = str_of(vm, &arg(a, 0))?;
                if sep == " " {
                    s.split_whitespace().map(String::from).collect()
                } else if sep.is_empty() {
                    s.chars().map(String::from).collect()
                } else {
                    let mut v: Vec<String> = s.split(sep.as_str()).map(String::from).collect();
                    while v.last().is_some_and(|x| x.is_empty()) {
                        v.pop();
                    }
                    v
                }
            }
        };
        Ok(str_array(vm, parts))
    });
    vm.def(&c, "ljust", |vm, s, a, _| pad(vm, s, a, 0));
    vm.def(&c, "rjust", |vm, s, a, _| pad(vm, s, a, 1));
    vm.def(&c, "center", |vm, s, a, _| pad(vm, s, a, 2));
    vm.def(&c, "[]", slice);
    vm.def(&c, "slice", slice);
    vm.def(&c, "succ", |vm, s, _, _| {
        Ok(vm.str(succ(&s_of(vm, s)?)))
    });
    vm.def(&c, "next", |vm, s, _, _| {
        Ok(vm.str(succ(&s_of(vm, s)?)))
    });
}

fn to_sym(vm: &mut Vm, s: &Value, _: &[Value], _: Option<&Value>) -> VmResult {
    let s = s_of(vm, s)?;
    Ok(Value::sym(vm.intern(&s)))
}

fn pad(vm: &mut Vm, s: &Value, a: &[Value], mode: u8) -> VmResult {
    let s = s_of(vm, s)?;
    let width = int_of(vm, &arg(a, 0))?.max(0) as usize;
    let padstr = if a.len() > 1 {
        str_of(vm, &a[1])?
    } else {
        " ".into()
    };
    let len = s.chars().count();
    if width <= len || padstr.is_empty() {
        return Ok(vm.str(s));
    }
    let fill = |n: usize| padstr.chars().cycle().take(n).collect::<String>();
    let total = width - len;
    Ok(vm.str(match mode {
        0 => s + &fill(total),
        1 => fill(total) + &s,
        _ => fill(total / 2) + &s + &fill(total - total / 2),
    }))
}

fn slice(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    let chars: Vec<char> = s_of(vm, s)?.chars().collect();
    let len = chars.len();
    let sub = |start: usize, n: usize| chars[start..(start + n).min(len)].iter().collect::<String>();
    let first = match vm.kind(arg(a, 0)) {
        V::Float(f) if f.is_finite() => V::Int(f as i64),
        k => k,
    };
    match (first, a.len()) {
        (V::Int(i), 1) => Ok(match norm_index(i, len) {
            Some(i) if i < len => vm.str(chars[i].to_string()),
            _ => Value::NIL,
        }),
        (V::Int(i), 2) => {
            let n = int_of(vm, &a[1])?;
            Ok(match norm_index(i, len) {
                Some(i) if n >= 0 => vm.str(sub(i, n as usize)),
                _ => Value::NIL,
            })
        }
        (V::Range(r), _) => {
            let r = vm.heap[r];
            let (lo, hi) = (int_of(vm, &r.0)?, if r.1.is_nil() { -1 } else { int_of(vm, &r.1)? });
            let Some(start) = norm_index(lo, len) else {
                return Ok(Value::NIL);
            };
            let mut end = if hi < 0 { hi + len as i64 } else { hi };
            if !r.2 || r.1.is_nil() {
                end += 1;
            }
            Ok(vm.str(sub(start, (end - start as i64).max(0) as usize)))
        }
        (V::Str(needle), _) => {
            let needle = vm.heap[needle].borrow().clone();
            Ok(if chars.iter().collect::<String>().contains(&needle) {
                vm.str(needle)
            } else {
                Value::NIL
            })
        }
        _ => Err(vm.type_err("no implicit conversion into Integer")),
    }
}

/// `String#succ` for alphanumerics: "a" → "b", "az" → "ba", "zz" → "aaa".
fn succ(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let mut i = chars.len();
    loop {
        if i == 0 {
            let first = chars[0];
            let carry = if first.is_ascii_digit() {
                '1'
            } else if first.is_ascii_uppercase() {
                'A'
            } else {
                'a'
            };
            chars.insert(0, carry);
            break;
        }
        i -= 1;
        let (next, wrapped) = match chars[i] {
            'z' => ('a', true),
            'Z' => ('A', true),
            '9' => ('0', true),
            c if c.is_ascii_alphanumeric() => (char::from_u32(c as u32 + 1).unwrap(), false),
            c => (char::from_u32(c as u32 + 1).unwrap_or(c), false),
        };
        chars[i] = next;
        if !wrapped {
            break;
        }
    }
    chars.into_iter().collect()
}

/// The contents of a String argument; TypeError for anything else.
fn str_arg(vm: &Vm, v: &Value) -> Result<String, Unwind> {
    match vm.kind(*v) {
        V::Str(h) => Ok(vm.heap[h].borrow().clone()),
        _ => Err(vm.type_err(format!("no implicit conversion of {} into String", vm.class_name(v)))),
    }
}

/// An index argument (an Integer, or a Float truncated).
fn index_arg(vm: &Vm, v: &Value) -> Result<i64, Unwind> {
    match vm.kind(*v) {
        V::Int(i) => Ok(i),
        V::Float(f) => Ok(f as i64),
        _ => Err(vm.type_err(format!("no implicit conversion of {} into Integer", vm.class_name(v)))),
    }
}

fn set_contents(vm: &mut Vm, s: &Value, text: String) -> Result<(), Unwind> {
    *vm.heap[this_str(vm, s)?].borrow_mut() = text;
    Ok(())
}

/// A bang method: what the method without `!` answers (with the same
/// arguments and block) replaces the contents. nil when that changes
/// nothing, unless `always_self` (`reverse!`, `succ!`).
fn bang(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>, name: &str, always_self: bool) -> VmResult {
    vm.check_frozen(s)?;
    let mid = vm.intern(name);
    let r = vm.call_method(s, mid, a.to_vec(), b.cloned())?;
    let new = str_of(vm, &r)?;
    if *vm.heap[this_str(vm, s)?].borrow() == new {
        return Ok(if always_self { *s } else { Value::NIL });
    }
    set_contents(vm, s, new)?;
    Ok(*s)
}

/// `s[i] = t`, `s[i, n] = t`, `s[range] = t`, `s[str] = t` (characters).
fn aset(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    vm.check_frozen(s)?;
    check_argc(vm, a, 2, 3)?;
    let val = a[a.len() - 1];
    let rep = str_arg(vm, &val)?;
    let mut chars: Vec<char> = s_of(vm, s)?.chars().collect();
    let len = chars.len() as i64;
    let out_of = |vm: &Vm, i: i64| vm.err(&vm.c.index_error, format!("index {i} out of string"));
    let (start, n) = match (vm.kind(a[0]), a.len()) {
        (V::Range(r), 2) => {
            let r = vm.heap[r];
            let lo = int_of(vm, &r.0)?;
            let hi = if r.1.is_nil() { -1 } else { int_of(vm, &r.1)? };
            let start = if lo < 0 { lo + len } else { lo };
            if !(0..=len).contains(&start) {
                let what = vm.inspect(&a[0])?;
                return Err(vm.err(&vm.c.range_error, format!("{what} out of range")));
            }
            let mut end = if hi < 0 { hi + len } else { hi };
            if !r.2 || r.1.is_nil() {
                end += 1;
            }
            (start, (end.min(len) - start).max(0))
        }
        (V::Str(needle), 2) => {
            let needle = vm.heap[needle].borrow().clone();
            let hay: String = chars.iter().collect();
            match hay.find(&needle) {
                Some(b) => (hay[..b].chars().count() as i64, needle.chars().count() as i64),
                None => return Err(vm.err(&vm.c.index_error, "string not matched")),
            }
        }
        (_, 2) => {
            let i = index_arg(vm, &a[0])?;
            let at = if i < 0 { i + len } else { i };
            if !(0..len).contains(&at) {
                return Err(out_of(vm, i));
            }
            (at, 1)
        }
        _ => {
            let i = index_arg(vm, &a[0])?;
            let n = index_arg(vm, &a[1])?;
            if n < 0 {
                return Err(vm.err(&vm.c.index_error, format!("negative length {n}")));
            }
            let at = if i < 0 { i + len } else { i };
            if !(0..=len).contains(&at) {
                return Err(out_of(vm, i));
            }
            (at, n.min(len - at))
        }
    };
    let (start, n) = (start as usize, n as usize);
    chars.splice(start..start + n, rep.chars());
    set_contents(vm, s, chars.into_iter().collect())?;
    Ok(val)
}

/// The character index of the last `needle` starting at or before `pos`.
fn rindex(vm: &mut Vm, s: &Value, a: &[Value], _: Option<&Value>) -> VmResult {
    check_argc(vm, a, 1, 2)?;
    let needle: Vec<char> = str_arg(vm, &a[0])?.chars().collect();
    let hay: Vec<char> = s_of(vm, s)?.chars().collect();
    let len = hay.len() as i64;
    let max = match a.get(1) {
        None => len,
        Some(p) => {
            let p = index_arg(vm, p)?;
            let p = if p < 0 { p + len } else { p };
            if p < 0 {
                return Ok(Value::NIL);
            }
            p.min(len)
        }
    };
    let last = (len - needle.len() as i64).min(max);
    for i in (0..=last).rev() {
        if hay[i as usize..].starts_with(&needle) {
            return Ok(Value::small_int(i));
        }
    }
    Ok(Value::NIL)
}

/// UTF-8 bytes back into a String (invalid sequences become U+FFFD:
/// Strings here are always UTF-8).
fn from_bytes(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn init_mutators(vm: &mut Vm) {
    let c = vm.c.string;
    vm.def(&c, "upcase!", |vm, s, a, b| bang(vm, s, a, b, "upcase", false));
    vm.def(&c, "downcase!", |vm, s, a, b| bang(vm, s, a, b, "downcase", false));
    vm.def(&c, "swapcase!", |vm, s, a, b| bang(vm, s, a, b, "swapcase", false));
    vm.def(&c, "capitalize!", |vm, s, a, b| bang(vm, s, a, b, "capitalize", false));
    vm.def(&c, "strip!", |vm, s, a, b| bang(vm, s, a, b, "strip", false));
    vm.def(&c, "lstrip!", |vm, s, a, b| bang(vm, s, a, b, "lstrip", false));
    vm.def(&c, "rstrip!", |vm, s, a, b| bang(vm, s, a, b, "rstrip", false));
    vm.def(&c, "chomp!", |vm, s, a, b| bang(vm, s, a, b, "chomp", false));
    vm.def(&c, "chop!", |vm, s, a, b| bang(vm, s, a, b, "chop", false));
    vm.def(&c, "squeeze!", |vm, s, a, b| bang(vm, s, a, b, "squeeze", false));
    vm.def(&c, "sub!", |vm, s, a, b| bang(vm, s, a, b, "sub", false));
    vm.def(&c, "gsub!", |vm, s, a, b| bang(vm, s, a, b, "gsub", false));
    vm.def(&c, "tr!", |vm, s, a, b| bang(vm, s, a, b, "tr", false));
    vm.def(&c, "delete!", |vm, s, a, b| bang(vm, s, a, b, "delete", false));
    vm.def(&c, "reverse!", |vm, s, a, b| bang(vm, s, a, b, "reverse", true));
    vm.def(&c, "succ!", |vm, s, a, b| bang(vm, s, a, b, "succ", true));
    vm.def(&c, "next!", |vm, s, a, b| bang(vm, s, a, b, "succ", true));
    vm.def(&c, "[]=", aset);
    vm.def(&c, "rindex", rindex);
    vm.def(&c, "init_copy", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let text = str_arg(vm, &arg(a, 0))?;
        set_contents(vm, s, text)?;
        Ok(*s)
    });
    // insert(i, t): before character i; a negative i counts from after the end
    vm.def(&c, "insert", |vm, s, a, _| {
        vm.check_frozen(s)?;
        check_argc(vm, a, 2, 2)?;
        let i = index_arg(vm, &a[0])?;
        let t = str_arg(vm, &a[1])?;
        let mut chars: Vec<char> = s_of(vm, s)?.chars().collect();
        let len = chars.len() as i64;
        let at = if i < 0 { i + len + 1 } else { i };
        if !(0..=len).contains(&at) {
            return Err(vm.err(&vm.c.index_error, format!("index {i} out of string")));
        }
        chars.splice(at as usize..at as usize, t.chars());
        set_contents(vm, s, chars.into_iter().collect())?;
        Ok(*s)
    });
    vm.def(&c, "prepend", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let mut front = String::new();
        for x in a {
            front.push_str(&str_arg(vm, x)?);
        }
        let text = front + &s_of(vm, s)?;
        set_contents(vm, s, text)?;
        Ok(*s)
    });
    vm.def(&c, "clear", |vm, s, _, _| {
        vm.check_frozen(s)?;
        set_contents(vm, s, String::new())?;
        Ok(*s)
    });
    // slice!(...): what s[...] answers, removed
    vm.def(&c, "slice!", |vm, s, a, b| {
        vm.check_frozen(s)?;
        let part = slice(vm, s, a, b)?;
        if !part.is_nil() {
            let mut args = a.to_vec();
            if let Some(V::Str(_)) = args.first().map(|x| vm.kind(*x)) {
                args.truncate(1);
            }
            let empty = vm.str(String::new());
            args.push(empty);
            aset(vm, s, &args, None)?;
        }
        Ok(part)
    });
    vm.def(&c, "chr", |vm, s, _, _| {
        let first = s_of(vm, s)?.chars().next().map(String::from).unwrap_or_default();
        Ok(vm.str(first))
    });

    // bytes (UTF-8)
    vm.def(&c, "getbyte", |vm, s, a, _| {
        let i = index_arg(vm, &arg(a, 0))?;
        let bytes = s_of(vm, s)?.into_bytes();
        Ok(norm_index(i, bytes.len())
            .and_then(|i| bytes.get(i))
            .map(|b| Value::small_int(*b as i64))
            .unwrap_or(Value::NIL))
    });
    vm.def(&c, "setbyte", |vm, s, a, _| {
        vm.check_frozen(s)?;
        check_argc(vm, a, 2, 2)?;
        let i = index_arg(vm, &a[0])?;
        let b = int_of(vm, &a[1])?;
        let mut bytes = s_of(vm, s)?.into_bytes();
        let Some(at) = norm_index(i, bytes.len()).filter(|&at| at < bytes.len()) else {
            return Err(vm.err(&vm.c.index_error, format!("index {i} out of string")));
        };
        bytes[at] = b as u8;
        set_contents(vm, s, from_bytes(&bytes))?;
        Ok(a[1])
    });
    vm.def(&c, "byteslice", |vm, s, a, _| {
        check_argc(vm, a, 1, 2)?;
        let bytes = s_of(vm, s)?.into_bytes();
        let len = bytes.len() as i64;
        let (start, n) = match (vm.kind(a[0]), a.len()) {
            (V::Range(r), 1) => {
                let r = vm.heap[r];
                let lo = int_of(vm, &r.0)?;
                let hi = if r.1.is_nil() { -1 } else { int_of(vm, &r.1)? };
                let start = if lo < 0 { lo + len } else { lo };
                let mut end = if hi < 0 { hi + len } else { hi };
                if !r.2 || r.1.is_nil() {
                    end += 1;
                }
                (start, (end - if start < 0 { start + len } else { start }).max(0))
            }
            (_, 1) => (index_arg(vm, &a[0])?, 1),
            _ => (index_arg(vm, &a[0])?, index_arg(vm, &a[1])?),
        };
        let start = if start < 0 { start + len } else { start };
        if start < 0 || start > len || n < 0 || (a.len() == 1 && start == len && !matches!(vm.kind(a[0]), V::Range(_))) {
            return Ok(Value::NIL);
        }
        let end = (start + n).min(len);
        Ok(vm.str(from_bytes(&bytes[start as usize..end.max(start) as usize])))
    });
    // bytesplice(index, length, str) / (range, str): bytes, on character boundaries
    vm.def(&c, "bytesplice", |vm, s, a, _| {
        vm.check_frozen(s)?;
        let text = s_of(vm, s)?;
        let len = text.len() as i64;
        let (start, n, rep) = match (a.len(), a.first().map(|x| vm.kind(*x))) {
            (2, Some(V::Range(r))) => {
                let r = vm.heap[r];
                let lo = int_of(vm, &r.0)?;
                let hi = if r.1.is_nil() { -1 } else { int_of(vm, &r.1)? };
                let start = if lo < 0 { lo + len } else { lo };
                let mut end = if hi < 0 { hi + len } else { hi };
                if !r.2 || r.1.is_nil() {
                    end += 1;
                }
                (start, (end.min(len) - start).max(0), str_arg(vm, &a[1])?)
            }
            (3, _) => (index_arg(vm, &a[0])?, index_arg(vm, &a[1])?, str_arg(vm, &a[2])?),
            _ => return Err(vm.arg_err(format!("wrong number of arguments (given {}, expected 2..3)", a.len()))),
        };
        let start = if start < 0 { start + len } else { start };
        if start < 0 || start > len {
            return Err(vm.err(&vm.c.index_error, format!("index {start} out of string")));
        }
        if n < 0 {
            return Err(vm.err(&vm.c.index_error, format!("negative length {n}")));
        }
        let (start, end) = (start as usize, (start + n).min(len) as usize);
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(vm.err(&vm.c.index_error, "offset doesn't land on character boundary"));
        }
        let new = format!("{}{}{}", &text[..start], rep, &text[end..]);
        set_contents(vm, s, new)?;
        Ok(*s)
    });
    vm.def(&c, "byteindex", |vm, s, a, _| {
        let needle = str_arg(vm, &arg(a, 0))?;
        let text = s_of(vm, s)?;
        let len = text.len() as i64;
        let from = match a.get(1) {
            Some(o) => index_arg(vm, o)?,
            None => 0,
        };
        let from = if from < 0 { from + len } else { from };
        if from < 0 || from > len {
            return Ok(Value::NIL);
        }
        Ok(text.as_bytes()[from as usize..]
            .windows(needle.len().max(1))
            .position(|w| needle.is_empty() || w == needle.as_bytes())
            .map(|i| Value::small_int(i as i64 + from))
            .or_else(|| (needle.is_empty() && from == len).then(|| Value::small_int(from)))
            .unwrap_or(Value::NIL))
    });
    vm.def(&c, "byterindex", |vm, s, a, _| {
        let needle = str_arg(vm, &arg(a, 0))?;
        let bytes = s_of(vm, s)?.into_bytes();
        let len = bytes.len() as i64;
        let max = match a.get(1) {
            Some(o) => {
                let o = index_arg(vm, o)?;
                let o = if o < 0 { o + len } else { o };
                if o < 0 {
                    return Ok(Value::NIL);
                }
                o.min(len)
            }
            None => len,
        };
        let last = (len - needle.len() as i64).min(max);
        for i in (0..=last).rev() {
            if bytes[i as usize..].starts_with(needle.as_bytes()) {
                return Ok(Value::small_int(i));
            }
        }
        Ok(Value::NIL)
    });
    vm.def(&c, "each_byte", |vm, s, _, b| {
        let b = need_block(vm, b)?;
        for byte in s_of(vm, s)?.into_bytes() {
            vm.call_block(&b, vec![Value::small_int(byte as i64)])?;
        }
        Ok(*s)
    });
}

/// `sub`/`gsub` with a String pattern: the replacement is a String (with
/// `\0`/`\&` for the match, `` \` `` and `\'` for what is before and after,
/// `\\` for a backslash), a Hash (looked up with the match), or the block's
/// value for the match. An empty pattern matches between characters.
fn substitute(vm: &mut Vm, s: &Value, a: &[Value], b: Option<&Value>, global: bool) -> VmResult {
    let text = s_of(vm, s)?;
    let pat = str_arg(vm, &arg(a, 0))?;
    let rep = a.get(1).copied();
    if rep.is_none() && b.is_none() {
        return Err(vm.arg_err("wrong number of arguments (given 1, expected 2)"));
    }
    let mut out = String::new();
    let mut at = 0;
    while at <= text.len() {
        let Some(found) = text[at..].find(&pat).map(|i| at + i) else {
            break;
        };
        out.push_str(&text[at..found]);
        let end = found + pat.len();
        match rep {
            Some(r) => match vm.kind(r) {
                V::Hash(h) => {
                    let k = vm.str(pat.clone());
                    let v = vm.hash_get(h, &k)?.unwrap_or(Value::NIL);
                    out.push_str(&vm.to_s(&v)?);
                }
                _ => {
                    let r = str_arg(vm, &r)?;
                    expand_backrefs(&mut out, &r, &text, found, end);
                }
            },
            None => {
                let m = vm.str(pat.clone());
                let v = vm.call_block(b.expect("checked"), vec![m])?;
                out.push_str(&vm.to_s(&v)?);
            }
        }
        if pat.is_empty() {
            // step over a character, keeping it
            match text[end..].chars().next() {
                Some(ch) => {
                    out.push(ch);
                    at = end + ch.len_utf8();
                }
                None => {
                    at = text.len() + 1;
                }
            }
        } else {
            at = end;
        }
        if !global {
            break;
        }
    }
    if at <= text.len() {
        out.push_str(&text[at..]);
    }
    Ok(vm.str(out))
}

fn expand_backrefs(out: &mut String, rep: &str, text: &str, start: usize, end: usize) {
    let mut chars = rep.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('0') | Some('&') => out.push_str(&text[start..end]),
            Some('`') => out.push_str(&text[..start]),
            Some('\'') => out.push_str(&text[end..]),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
}

