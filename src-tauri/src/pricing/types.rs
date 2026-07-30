#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageBreakdown {
    pub total_tokens: i64,
    /// Total input reported by OpenAI-compatible APIs; cache buckets are subsets.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
    /// A subset of output tokens. It is never added to the billable total again.
    pub reasoning_tokens: i64,
    pub model: Option<String>,
    pub service_tier: Option<String>,
}

impl UsageBreakdown {
    pub fn normalize(mut self) -> Self {
        self.input_tokens = self.input_tokens.max(0);
        self.output_tokens = self.output_tokens.max(0);
        self.cached_tokens = self.cached_tokens.max(0).min(self.input_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .max(0)
            .min(self.input_tokens.saturating_sub(self.cached_tokens));
        self.reasoning_tokens = self.reasoning_tokens.max(0).min(self.output_tokens);
        let component_total = self.input_tokens.saturating_add(self.output_tokens);
        self.total_tokens = self.total_tokens.max(component_total).max(0);
        self.model = self.model.and_then(trimmed_value);
        self.service_tier = self.service_tier.and_then(trimmed_value);
        self
    }
}

fn trimmed_value(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    pub total_cost: f64,
    pub unpriced_tokens: i64,
    pub matched_model: Option<&'static str>,
}
