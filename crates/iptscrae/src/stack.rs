//! The data stack.
//!
//! A plain `Vec` with an inclusive depth cap. Unlike the reference VM — where
//! exceeding 256 items quietly drops the oldest ("some of them will fall off",
//! per the guide) — overflow here is an error. Scripts are untrusted remote
//! code, so failing closed is the safer dialect; see the crate README.

use crate::error::{IptError, Result};
use crate::value::Value;

/// A LIFO stack of [`Value`]s.
#[derive(Debug, Clone)]
pub struct Stack {
    items: Vec<Value>,
    limit: usize,
}

impl Stack {
    /// An empty stack that will refuse to grow past `limit` items.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            items: Vec::new(),
            limit,
        }
    }

    /// Number of items currently on the stack.
    pub fn depth(&self) -> usize {
        self.items.len()
    }

    /// The configured maximum depth.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Push a value, failing if that would exceed the depth limit.
    pub fn push(&mut self, value: Value) -> Result<()> {
        if self.items.len() >= self.limit {
            return Err(IptError::StackOverflow { limit: self.limit });
        }
        self.items.push(value);
        Ok(())
    }

    /// Pop the top value, failing on an empty stack.
    pub fn pop(&mut self) -> Result<Value> {
        self.items.pop().ok_or(IptError::StackUnderflow {
            needed: 1,
            available: 0,
        })
    }

    /// Pop the top value if there is one.
    pub fn pop_opt(&mut self) -> Option<Value> {
        self.items.pop()
    }

    /// The `n`-th item from the top, where `0` is the top item.
    pub fn peek(&self, n: usize) -> Result<&Value> {
        let depth = self.items.len();
        if n >= depth {
            return Err(IptError::StackUnderflow {
                needed: n + 1,
                available: depth,
            });
        }
        Ok(&self.items[depth - 1 - n])
    }

    /// Consume the stack, returning items bottom-first.
    pub fn into_items(self) -> Vec<Value> {
        self.items
    }

    /// Remove and return every item, top-first.
    pub fn drain_top_first(&mut self) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.items.len());
        while let Some(v) = self.items.pop() {
            out.push(v);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn push_pop_and_depth() {
        let mut s = Stack::with_limit(4);
        s.push(Value::Int(1)).unwrap();
        s.push(Value::Int(2)).unwrap();
        assert_eq!(s.depth(), 2);
        assert_eq!(s.pop().unwrap(), Value::Int(2));
        assert_eq!(s.pop().unwrap(), Value::Int(1));
        assert!(s.is_empty());
    }

    #[test]
    fn peek_is_zero_based_from_the_top() {
        let mut s = Stack::with_limit(4);
        s.push(Value::Int(10)).unwrap();
        s.push(Value::Int(20)).unwrap();
        s.push(Value::Int(30)).unwrap();
        assert_eq!(s.peek(0).unwrap(), &Value::Int(30));
        assert_eq!(s.peek(2).unwrap(), &Value::Int(10));
        assert!(s.peek(3).is_err());
    }

    #[test]
    fn overflow_and_underflow_are_errors_not_panics() {
        let mut s = Stack::with_limit(2);
        s.push(Value::Int(1)).unwrap();
        s.push(Value::Int(2)).unwrap();
        assert_eq!(
            s.push(Value::Int(3)),
            Err(IptError::StackOverflow { limit: 2 })
        );
        let mut e = Stack::with_limit(2);
        assert!(matches!(e.pop(), Err(IptError::StackUnderflow { .. })));
    }

    #[test]
    fn values_share_handles() {
        let a = Value::array(vec![Value::Int(1)]);
        let b = a.clone();
        if let Value::Array(cell) = &b {
            cell.borrow_mut().push(Value::Int(2));
        }
        assert_eq!(a.array_len(), 2, "clone is a handle, not a copy");
        assert_eq!(Value::Var(Rc::from("X")), Value::Var(Rc::from("X")));
    }
}
