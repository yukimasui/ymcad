//! 道具の引数の取り出し。説明は日本語で返す（LLM が読んで直せるように）。
//!
//! 値が `null` の引数は「指定なし」として扱う。

use serde_json::{Map, Value};

/// 道具の引数。
#[derive(Debug)]
pub(crate) struct Args(Map<String, Value>);

impl Args {
    /// 引数を受け取る。**`known` に無い引数があれば拒む**。
    pub(crate) fn new(map: Map<String, Value>, known: &[&str]) -> Result<Self, String> {
        let unknown: Vec<&str> = map
            .keys()
            .map(String::as_str)
            .filter(|k| !known.contains(k))
            .collect();
        if !unknown.is_empty() {
            let accepted = if known.is_empty() {
                "この道具は引数を取りません".to_owned()
            } else {
                format!("使えるのは {}", known.join(", "))
            };
            return Err(format!(
                "知らない引数があります: {}（{accepted}）",
                unknown.join(", ")
            ));
        }
        Ok(Self(map))
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    /// 生の値（指定なしなら `None`）。
    pub(crate) fn value(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }

    /// 真偽。指定なしなら `default`。
    pub(crate) fn bool_or(&self, key: &str, default: bool) -> Result<bool, String> {
        match self.get(key) {
            None => Ok(default),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| format!("{key} は true か false で指定してください")),
        }
    }

    /// 文字列（任意）。
    pub(crate) fn opt_str(&self, key: &str) -> Result<Option<&str>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(Some)
                .ok_or_else(|| format!("{key} は文字列で指定してください")),
        }
    }

    /// 文字列（必須）。
    pub(crate) fn req_str(&self, key: &str) -> Result<&str, String> {
        self.opt_str(key)?
            .ok_or_else(|| format!("{key} を指定してください"))
    }

    /// `min..=max` の整数。指定なしなら `default`。
    pub(crate) fn usize_in(
        &self,
        key: &str,
        default: usize,
        min: usize,
        max: usize,
    ) -> Result<usize, String> {
        let Some(v) = self.get(key) else {
            return Ok(default);
        };
        let n = v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (min..=max).contains(n));
        n.ok_or_else(|| format!("{key} は {min} 以上 {max} 以下の整数で指定してください"))
    }

    /// 文字列の配列（必須・1 個以上 `max` 個以下）。
    pub(crate) fn string_list(&self, key: &str, max: usize) -> Result<Vec<String>, String> {
        let list = self
            .get(key)
            .ok_or_else(|| format!("{key} を指定してください"))?
            .as_array()
            .ok_or_else(|| format!("{key} は文字列の配列で指定してください"))?;
        if list.is_empty() || list.len() > max {
            return Err(format!("{key} は 1 個以上 {max} 個以下で指定してください"));
        }
        list.iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key} の要素は文字列で指定してください"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Args {
        let Value::Object(m) = v else { unreachable!() };
        Args::new(m, &["a", "b", "n", "list"]).unwrap()
    }

    #[test]
    fn unknown_keys_are_listed() {
        let Value::Object(m) = json!({"a": 1, "zz": 2}) else {
            unreachable!()
        };
        let e = Args::new(m, &["a"]).unwrap_err();
        assert!(e.contains("zz") && e.contains("使えるのは a"), "{e}");
    }

    #[test]
    fn null_means_not_given() {
        let a = args(json!({"a": null, "n": null}));
        assert!(!a.bool_or("a", false).unwrap());
        assert_eq!(a.usize_in("n", 7, 1, 10).unwrap(), 7);
        assert!(a.req_str("a").is_err());
    }

    #[test]
    fn integers_are_range_checked() {
        let a = args(json!({"n": 11, "a": -1, "b": 2.5}));
        assert!(a.usize_in("n", 1, 1, 10).is_err());
        assert!(a.usize_in("a", 1, 1, 10).is_err());
        assert!(a.usize_in("b", 1, 1, 10).is_err());
        assert_eq!(args(json!({"n": 10})).usize_in("n", 1, 1, 10).unwrap(), 10);
    }

    #[test]
    fn string_lists_are_bounded() {
        assert!(args(json!({"list": []})).string_list("list", 2).is_err());
        assert!(args(json!({"list": ["x", "y", "z"]}))
            .string_list("list", 2)
            .is_err());
        assert!(args(json!({"list": ["x", 1]}))
            .string_list("list", 2)
            .is_err());
        assert_eq!(
            args(json!({"list": ["x", "y"]}))
                .string_list("list", 2)
                .unwrap(),
            vec!["x", "y"]
        );
    }

    #[test]
    fn wrong_types_are_explained() {
        let a = args(json!({"a": "yes", "b": 3}));
        assert!(a.bool_or("a", false).unwrap_err().contains("true か false"));
        assert!(a.opt_str("b").unwrap_err().contains("文字列"));
    }
}
