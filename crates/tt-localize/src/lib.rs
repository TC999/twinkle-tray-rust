//! 本地化运行时。
//!
//! 等价移植 `origin/src/Translate.js` 与 `origin/src/electron.js` 的
//! `getLocalization()`（equiv: electron.js:L2241-L2291）。
//! FTL 产物由 `tools/convert-localization.mjs` 从 JSON 语料生成（AGENTS.md §2）。

use std::collections::HashMap;
use std::sync::OnceLock;

/// 单条本地化消息表：KEY -> 原文（含 {{N}} 占位符，原样保留）。
type Messages<'a> = HashMap<&'a str, &'a str>;

static EN: OnceLock<Messages<'static>> = OnceLock::new();

fn load_en() -> &'static Messages<'static> {
    EN.get_or_init(|| parse_ftl(include_str!("../../../src/localization/en.ftl")))
}

/// 运行时同款解析：按首个 ` = ` 切分；忽略 `#` 注释与空行。
fn parse_ftl<'a>(text: &'a str) -> Messages<'a> {
    let mut map = HashMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(idx) = line.find(" = ") {
            map.insert(&line[..idx], &line[idx + 3..]);
        }
    }
    map
}

/// 等价 Translate.js `makeTranslation`：按 {{1}}, {{2}}, ... 顺序替换。
/// args 耗尽后剩余占位符原样保留（原行为如此，不得"修复"）。
pub fn make_translation(string: &str, args: &[&str]) -> String {
    let mut out = string.to_string();
    for (i, arg) in args.iter().enumerate() {
        let ph = format!("{{{{{}}}}}", i + 1);
        out = out.replacen(&ph, arg, 1);
    }
    out
}

/// 等价 Translate.js `getString`：desired 缺键或空值 -> en 回退 -> 仍无则空串。
pub fn get_string_with(desired: &Messages<'_>, key: &str, args: &[&str]) -> String {
    if let Some(v) = desired.get(key) {
        if !v.is_empty() {
            return make_translation(v, args);
        }
    }
    let fallback = load_en();
    if let Some(v) = fallback.get(key) {
        if !v.is_empty() {
            return make_translation(v, args);
        }
    }
    String::new()
}

/// 语言检测（equiv: electron.js:L2242-L2253）：
/// zh-CN -> zh-Hans；zh-TW/HK/MO -> zh-Hant；pt-* 保留完整 locale；其余取 `-` 前缀。
pub fn detect_language_code(locale: &str) -> String {
    if locale == "zh-CN" {
        "zh-Hans".to_string()
    } else if locale == "zh-TW" || locale == "zh-HK" || locale == "zh-MO" {
        "zh-Hant".to_string()
    } else if locale.split('-').next() == Some("pt") {
        locale.to_string()
    } else {
        locale.split('-').next().unwrap_or("").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_order_preserved() {
        // args 耗尽后剩余占位符原样保留
        assert_eq!(make_translation("A {{1}} B {{2}}", &["X"]), "A X B {{2}}");
        assert_eq!(
            make_translation("Have Twinkle Tray {{1}}.", &["1.18.0"]),
            "Have Twinkle Tray 1.18.0."
        );
    }

    #[test]
    fn fallback_chain() {
        let empty: Messages<'_> = HashMap::new();
        // zh-Hans 缺 GENERIC_OK? en 必有
        assert_eq!(get_string_with(&empty, "GENERIC_OK", &[]), "OK");
        // 未知键 -> 空串
        assert_eq!(get_string_with(&empty, "NO_SUCH_KEY_ZZZ", &[]), "");
    }

    #[test]
    fn zh_hans_available() {
        let zh = parse_ftl(include_str!("../../../src/localization/zh-Hans.ftl"));
        assert_eq!(zh.get("GENERIC_OK"), Some(&"确定"));
        assert_eq!(zh.len(), 253);
    }

    #[test]
    fn detection() {
        assert_eq!(detect_language_code("zh-CN"), "zh-Hans");
        assert_eq!(detect_language_code("zh-TW"), "zh-Hant");
        assert_eq!(detect_language_code("pt-BR"), "pt-BR");
        assert_eq!(detect_language_code("en-US"), "en");
    }
}
