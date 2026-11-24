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


// ---- 全局当前语言（equiv Translate.js 的 localization.desired）----

use std::sync::Mutex;

static CURRENT: Mutex<Option<(String, Messages<'static>)>> = Mutex::new(None);

/// 语言代码 -> FTL 语料（38 种语言，与 src/localization/*.ftl 一一对应）。
pub fn language_source(code: &str) -> Option<&'static str> {
    let src = match code {
        "ar" => include_str!("../../../src/localization/ar.ftl"),
        "az" => include_str!("../../../src/localization/az.ftl"),
        "bn" => include_str!("../../../src/localization/bn.ftl"),
        "ckb" => include_str!("../../../src/localization/ckb.ftl"),
        "cs" => include_str!("../../../src/localization/cs.ftl"),
        "de" => include_str!("../../../src/localization/de.ftl"),
        "el" => include_str!("../../../src/localization/el.ftl"),
        "en" => include_str!("../../../src/localization/en.ftl"),
        "en-GB" => include_str!("../../../src/localization/en-GB.ftl"),
        "es" => include_str!("../../../src/localization/es.ftl"),
        "fa" => include_str!("../../../src/localization/fa.ftl"),
        "fi" => include_str!("../../../src/localization/fi.ftl"),
        "fr" => include_str!("../../../src/localization/fr.ftl"),
        "he" => include_str!("../../../src/localization/he.ftl"),
        "hi" => include_str!("../../../src/localization/hi.ftl"),
        "hr" => include_str!("../../../src/localization/hr.ftl"),
        "hu" => include_str!("../../../src/localization/hu.ftl"),
        "id" => include_str!("../../../src/localization/id.ftl"),
        "it" => include_str!("../../../src/localization/it.ftl"),
        "ja" => include_str!("../../../src/localization/ja.ftl"),
        "ko" => include_str!("../../../src/localization/ko.ftl"),
        "lt" => include_str!("../../../src/localization/lt.ftl"),
        "nb" => include_str!("../../../src/localization/nb.ftl"),
        "nl" => include_str!("../../../src/localization/nl.ftl"),
        "pl" => include_str!("../../../src/localization/pl.ftl"),
        "pt" => include_str!("../../../src/localization/pt.ftl"),
        "pt-BR" => include_str!("../../../src/localization/pt-BR.ftl"),
        "ro" => include_str!("../../../src/localization/ro.ftl"),
        "ru" => include_str!("../../../src/localization/ru.ftl"),
        "sk" => include_str!("../../../src/localization/sk.ftl"),
        "sv" => include_str!("../../../src/localization/sv.ftl"),
        "ta" => include_str!("../../../src/localization/ta.ftl"),
        "th" => include_str!("../../../src/localization/th.ftl"),
        "tr" => include_str!("../../../src/localization/tr.ftl"),
        "uk" => include_str!("../../../src/localization/uk.ftl"),
        "vi" => include_str!("../../../src/localization/vi.ftl"),
        "zh-Hans" => include_str!("../../../src/localization/zh-Hans.ftl"),
        "zh-Hant" => include_str!("../../../src/localization/zh-Hant.ftl"),
        _ => return None,
    };
    Some(src)
}

/// 设置当前语言（equiv getLocalization 的 desired 装载）。
pub fn set_language(code: &str) {
    let parsed = language_source(code).map(parse_ftl);
    if let Ok(mut guard) = CURRENT.lock() {
        *guard = parsed.map(|m| (code.to_string(), m));
    }
}

/// 当前语言代码（未设置时为 en）。
pub fn current_language() -> String {
    CURRENT
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|(c, _)| c.clone()))
        .unwrap_or_else(|| "en".to_string())
}

/// t(key)：用当前语言表 + en 回退链（equiv Translate.getString）。
pub fn t(key: &str) -> String {
    let guard = CURRENT.lock().ok();
    match guard.as_ref().and_then(|g| g.as_ref()) {
        Some((_, messages)) => get_string_with(messages, key, &[]),
        None => get_string_with(&Messages::new(), key, &[]),
    }
}

/// t_args(key, args)：带占位符替换。
pub fn t_args(key: &str, args: &[&str]) -> String {
    let guard = CURRENT.lock().ok();
    match guard.as_ref().and_then(|g| g.as_ref()) {
        Some((_, messages)) => get_string_with(messages, key, args),
        None => get_string_with(&Messages::new(), key, args),
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
        assert_eq!(zh.len(), 255);
    }

    #[test]
    fn detection() {
        assert_eq!(detect_language_code("zh-CN"), "zh-Hans");
        assert_eq!(detect_language_code("zh-TW"), "zh-Hant");
        assert_eq!(detect_language_code("pt-BR"), "pt-BR");
        assert_eq!(detect_language_code("en-US"), "en");
    }
}