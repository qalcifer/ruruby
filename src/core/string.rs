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
    let c = vm.c.string;
    // String.new(str), and a subclass's `super` in initialize
    vm.def(&c, "initialize", |vm, s, a, _| {
        check_argc(vm, a, 0, 1)?;
        if let [x] = a {
            let text = str_of(vm, x)?;
            *vm.heap[this_str(vm, s)?].borrow_mut() = text;
        }
        Ok(Value::NIL)
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
    vm.def(&c, "to_i", |vm, s, _, _| {
        let s = s_of(vm, s)?;
        let t = s.trim_start();
        let mut end = 0;
        for (i, ch) in t.char_indices() {
            if ch.is_ascii_digit() || ch == '_' || (i == 0 && (ch == '-' || ch == '+')) {
                end = i + ch.len_utf8();
            } else {
                break;
            }
        }
        Ok(vm.int(t[..end].replace('_', "").parse().unwrap_or(0)))
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
        let n = int_of(vm, &arg(a, 0))?;
        if n < 0 {
            return Err(vm.arg_err("negative argument"));
        }
        Ok(vm.str(s_of(vm, s)?.repeat(n as usize)))
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
        let piece = vm.to_s(&arg(a, 0))?;
        vm.heap[this_str(vm, s)?].borrow_mut().push_str(&piece);
        Ok(*s)
    };
    vm.def(&c, "<<", append);
    vm.def(&c, "concat", append);
    vm.def(&c, "replace", |vm, s, a, _| {
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
    vm.def(&c, "index", |vm, s, a, _| {
        let s = s_of(vm, s)?;
        let needle = str_of(vm, &arg(a, 0))?;
        Ok(s.find(&needle)
            .map(|b| Value::small_int(s[..b].chars().count() as i64))
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
    vm.def(&c, "sub", |vm, s, a, _| {
        let (pat, rep) = (str_of(vm, &arg(a, 0))?, str_of(vm, &arg(a, 1))?);
        Ok(vm.str(s_of(vm, s)?.replacen(&pat, &rep, 1)))
    });
    vm.def(&c, "gsub", |vm, s, a, _| {
        let (pat, rep) = (str_of(vm, &arg(a, 0))?, str_of(vm, &arg(a, 1))?);
        Ok(vm.str(s_of(vm, s)?.replace(&pat, &rep)))
    });
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
    match (vm.kind(arg(a, 0)), a.len()) {
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
