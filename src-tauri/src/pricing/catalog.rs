#[derive(Clone, Copy)]
pub(super) struct ModelPrice {
    pub(super) model: &'static str,
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) cache_read: Option<f64>,
    pub(super) cache_write: Option<f64>,
    pub(super) priority_multiplier: Option<f64>,
    pub(super) flex_multiplier: Option<f64>,
    pub(super) long_context: bool,
}

pub(super) const USD_PER_MILLION: f64 = 0.000_001;

macro_rules! price {
    ($model:literal, $input:expr, $output:expr, $cache:expr, $priority:expr, $flex:expr, $long:expr) => {
        ModelPrice {
            model: $model,
            input: $input * USD_PER_MILLION,
            output: $output * USD_PER_MILLION,
            cache_read: Some($cache * USD_PER_MILLION),
            cache_write: None,
            priority_multiplier: $priority,
            flex_multiplier: $flex,
            long_context: $long,
        }
    };
}

// USD prices per token, expressed below as USD per million tokens for readability.
// Exact model names are intentional: an unknown future model remains unpriced.
const MODEL_PRICES: &[ModelPrice] = &[
    ModelPrice {
        model: "gpt-5.6-sol",
        input: 5.0 * USD_PER_MILLION,
        output: 30.0 * USD_PER_MILLION,
        cache_read: Some(0.5 * USD_PER_MILLION),
        cache_write: Some(6.25 * USD_PER_MILLION),
        priority_multiplier: Some(2.0),
        flex_multiplier: Some(0.5),
        long_context: true,
    },
    ModelPrice {
        model: "gpt-5.6-terra",
        input: 2.5 * USD_PER_MILLION,
        output: 15.0 * USD_PER_MILLION,
        cache_read: Some(0.25 * USD_PER_MILLION),
        cache_write: Some(3.125 * USD_PER_MILLION),
        priority_multiplier: Some(2.0),
        flex_multiplier: Some(0.5),
        long_context: true,
    },
    ModelPrice {
        model: "gpt-5.6-luna",
        input: 1.0 * USD_PER_MILLION,
        output: 6.0 * USD_PER_MILLION,
        cache_read: Some(0.1 * USD_PER_MILLION),
        cache_write: Some(1.25 * USD_PER_MILLION),
        priority_multiplier: Some(2.0),
        flex_multiplier: Some(0.5),
        long_context: true,
    },
    price!("gpt-5.5-pro", 30.0, 180.0, 3.0, Some(2.0), Some(0.5), true),
    price!("gpt-5.5", 5.0, 30.0, 0.5, Some(2.0), Some(0.5), true),
    price!("gpt-5.4-pro", 30.0, 180.0, 3.0, Some(2.0), Some(0.5), true),
    price!(
        "gpt-5.4-mini",
        0.75,
        4.5,
        0.075,
        Some(2.0),
        Some(0.5),
        false
    ),
    price!("gpt-5.4-nano", 0.2, 1.25, 0.02, None, Some(0.5), false),
    price!("gpt-5.4", 2.5, 15.0, 0.25, Some(2.0), Some(0.5), true),
    price!(
        "gpt-5.3-chat-latest",
        1.75,
        14.0,
        0.175,
        Some(2.0),
        None,
        false
    ),
    price!(
        "gpt-5.3-codex-spark",
        1.75,
        14.0,
        0.175,
        Some(2.0),
        None,
        false
    ),
    price!("gpt-5.3-codex", 1.75, 14.0, 0.175, Some(2.0), None, false),
    price!("gpt-5.2-pro", 21.0, 168.0, 21.0, Some(2.0), None, false),
    price!("gpt-5.2-codex", 1.75, 14.0, 0.175, Some(2.0), None, false),
    price!(
        "gpt-5.2-chat-latest",
        1.75,
        14.0,
        0.175,
        Some(2.0),
        None,
        false
    ),
    price!("gpt-5.2", 1.75, 14.0, 0.175, Some(2.0), None, false),
    price!(
        "gpt-5.1-codex-mini",
        0.25,
        2.0,
        0.025,
        Some(1.8),
        None,
        false
    ),
    price!(
        "gpt-5.1-codex-max",
        1.25,
        10.0,
        0.125,
        Some(2.0),
        None,
        false
    ),
    price!("gpt-5.1-codex", 1.25, 10.0, 0.125, Some(2.0), None, false),
    price!(
        "gpt-5.1-chat-latest",
        1.25,
        10.0,
        0.125,
        Some(2.0),
        None,
        false
    ),
    price!("gpt-5.1", 1.25, 10.0, 0.125, Some(2.0), None, false),
    price!(
        "gpt-5-codex",
        1.25,
        10.0,
        0.125,
        Some(2.0),
        Some(0.5),
        false
    ),
    price!("gpt-5-mini", 0.25, 2.0, 0.025, Some(1.8), Some(0.5), false),
    price!("gpt-5-nano", 0.05, 0.4, 0.005, None, Some(0.5), false),
    price!("gpt-5-pro", 15.0, 120.0, 15.0, Some(2.0), None, false),
    price!("gpt-5", 1.25, 10.0, 0.125, Some(2.0), Some(0.5), false),
    price!(
        "o4-mini",
        1.1,
        4.4,
        0.275,
        Some(20.0 / 11.0),
        Some(0.5),
        false
    ),
    price!("o3-mini", 1.1, 4.4, 0.55, Some(1.75), Some(0.5), false),
    price!("o3-pro", 20.0, 80.0, 20.0, None, None, false),
    price!("o3", 2.0, 8.0, 0.5, Some(1.75), Some(0.5), false),
    price!("o1-pro", 150.0, 600.0, 150.0, None, None, false),
    price!("gpt-4.1-mini", 0.4, 1.6, 0.1, Some(1.75), None, false),
    price!("gpt-4.1-nano", 0.1, 0.4, 0.025, Some(1.75), None, false),
    price!("gpt-4.1", 2.0, 8.0, 0.5, Some(1.75), None, false),
    price!("gpt-4o-mini", 0.15, 0.6, 0.075, Some(1.7), None, false),
    price!("gpt-4o", 2.5, 10.0, 1.25, Some(1.7), None, false),
];

pub(super) fn find_model_price(model: &str) -> Option<&'static ModelPrice> {
    let canonical = canonical_model(model)?;
    MODEL_PRICES.iter().find(|price| price.model == canonical)
}

fn canonical_model(model: &str) -> Option<String> {
    let mut normalized = model
        .trim()
        .to_ascii_lowercase()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .replace(['_', ' '], "-");
    while normalized.contains("--") {
        normalized = normalized.replace("--", "-");
    }
    if normalized == "gpt5.6" {
        normalized = "gpt-5.6".to_string();
    }
    normalized = strip_snapshot_suffix(&normalized).to_string();

    if MODEL_PRICES.iter().any(|price| price.model == normalized) {
        return Some(normalized);
    }

    const EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
    if normalized == "gpt-5.6"
        || EFFORTS
            .iter()
            .any(|effort| normalized == format!("gpt-5.6-{effort}"))
    {
        return Some("gpt-5.6-sol".to_string());
    }
    for family in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        if EFFORTS
            .iter()
            .any(|effort| normalized == format!("{family}-{effort}"))
        {
            return Some(family.to_string());
        }
    }
    if EFFORTS
        .iter()
        .any(|effort| normalized == format!("gpt-5.4-{effort}"))
    {
        return Some("gpt-5.4".to_string());
    }
    if EFFORTS
        .iter()
        .any(|effort| normalized == format!("gpt-5.3-{effort}"))
    {
        return Some("gpt-5.3-codex".to_string());
    }
    None
}

fn strip_snapshot_suffix(model: &str) -> &str {
    if model.len() < 11 {
        return model;
    }
    let suffix = &model[model.len() - 11..];
    let bytes = suffix.as_bytes();
    let is_date = bytes[0] == b'-'
        && bytes[1..5].iter().all(u8::is_ascii_digit)
        && bytes[5] == b'-'
        && bytes[6..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'-'
        && bytes[9..11].iter().all(u8::is_ascii_digit);
    if is_date {
        &model[..model.len() - 11]
    } else {
        model
    }
}
