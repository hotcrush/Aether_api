//! API-equivalent token cost estimation.
//!
//! The bucket model follows the same accounting shape used by Sub2API and LiteLLM,
//! while the matching and calculation code here is implemented locally. Prices are
//! a bundled snapshot and are estimates rather than an upstream invoice.

mod calculator;
mod catalog;
mod types;

pub use calculator::estimate_cost;
pub use types::{CostEstimate, UsageBreakdown};

pub const PRICING_UPDATED_AT: &str = "2026-07-31";
pub const PRICING_SOURCE: &str = "LiteLLM / Sub2API pricing snapshot";

#[cfg(test)]
mod tests {
    use super::catalog::USD_PER_MILLION;
    use super::*;

    fn usage(model: &str) -> UsageBreakdown {
        UsageBreakdown {
            total_tokens: 1_800,
            input_tokens: 1_500,
            output_tokens: 300,
            cached_tokens: 400,
            cache_write_tokens: 100,
            reasoning_tokens: 200,
            model: Some(model.to_string()),
            service_tier: None,
        }
    }

    #[test]
    fn sol_uses_separate_buckets_without_double_billing_reasoning() {
        let estimate = estimate_cost(&usage("openai/gpt-5.6-sol"), None);
        let expected = 1_000.0 * 5.0 * USD_PER_MILLION
            + 400.0 * 0.5 * USD_PER_MILLION
            + 100.0 * 6.25 * USD_PER_MILLION
            + 300.0 * 30.0 * USD_PER_MILLION;
        assert!((estimate.total_cost - expected).abs() < 1e-12);
        assert_eq!(estimate.unpriced_tokens, 0);
        assert_eq!(estimate.matched_model, Some("gpt-5.6-sol"));
    }

    #[test]
    fn sol_long_context_prices_the_whole_request() {
        let mut usage = usage("gpt-5.6-high");
        usage.total_tokens = 300_100;
        usage.input_tokens = 300_000;
        usage.output_tokens = 100;
        usage.cached_tokens = 0;
        usage.cache_write_tokens = 0;
        let estimate = estimate_cost(&usage, None);
        let expected =
            300_000.0 * 5.0 * USD_PER_MILLION * 2.0 + 100.0 * 30.0 * USD_PER_MILLION * 1.5;
        assert!((estimate.total_cost - expected).abs() < 1e-12);
    }

    #[test]
    fn luna_and_terra_use_the_v0169_rates() {
        let terra = estimate_cost(&usage("gpt-5.6-terra"), None);
        let terra_expected =
            (1_000.0 * 2.0 + 400.0 * 0.2 + 100.0 * 2.5 + 300.0 * 12.0) * USD_PER_MILLION;
        assert!((terra.total_cost - terra_expected).abs() < 1e-12);

        let luna = estimate_cost(&usage("gpt-5.6-luna"), None);
        let luna_expected =
            (1_000.0 * 0.2 + 400.0 * 0.02 + 100.0 * 0.25 + 300.0 * 1.2) * USD_PER_MILLION;
        assert!((luna.total_cost - luna_expected).abs() < 1e-12);
    }

    #[test]
    fn snapshot_aliases_match_but_unknown_models_do_not() {
        assert_eq!(
            estimate_cost(&usage("gpt-5.4-2026-03-05"), None).matched_model,
            Some("gpt-5.4")
        );
        let unknown = estimate_cost(&usage("gpt-5.99-future"), None);
        assert_eq!(unknown.total_cost, 0.0);
        assert_eq!(unknown.unpriced_tokens, 1_800);
        assert_eq!(unknown.matched_model, None);
    }
}
