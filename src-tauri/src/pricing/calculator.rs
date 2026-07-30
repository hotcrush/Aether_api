use super::catalog::find_model_price;
use super::{CostEstimate, UsageBreakdown};

pub fn estimate_cost(usage: &UsageBreakdown, model_hint: Option<&str>) -> CostEstimate {
    let usage = usage.clone().normalize();
    let model = usage.model.as_deref().or(model_hint);
    let Some(price) = model.and_then(find_model_price) else {
        return CostEstimate {
            total_cost: 0.0,
            unpriced_tokens: usage.total_tokens,
            matched_model: None,
        };
    };

    let component_total = usage.input_tokens.saturating_add(usage.output_tokens);
    if component_total <= 0 {
        return CostEstimate {
            total_cost: 0.0,
            unpriced_tokens: usage.total_tokens,
            matched_model: Some(price.model),
        };
    }

    let cached = usage.cached_tokens.min(usage.input_tokens);
    let cache_write = usage
        .cache_write_tokens
        .min(usage.input_tokens.saturating_sub(cached));
    let uncached_input = usage
        .input_tokens
        .saturating_sub(cached)
        .saturating_sub(cache_write);
    let tier_multiplier = match usage
        .service_tier
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "priority" => price.priority_multiplier.unwrap_or(1.0),
        "flex" | "batch" => price.flex_multiplier.unwrap_or(1.0),
        _ => 1.0,
    };
    let long_context = price.long_context && usage.input_tokens > 272_000;
    let long_input_multiplier = if long_context { 2.0 } else { 1.0 };
    let long_output_multiplier = if long_context { 1.5 } else { 1.0 };
    let cache_read_price = price.cache_read.unwrap_or(price.input);
    let cache_write_price = price.cache_write.unwrap_or(price.input);
    let total_cost = tier_multiplier
        * (long_input_multiplier
            * (uncached_input as f64 * price.input
                + cached as f64 * cache_read_price
                + cache_write as f64 * cache_write_price)
            + long_output_multiplier * usage.output_tokens as f64 * price.output);

    CostEstimate {
        total_cost,
        unpriced_tokens: usage.total_tokens.saturating_sub(component_total),
        matched_model: Some(price.model),
    }
}
