use super::types::{MarketCategoryMetric, MarketProduct};
use std::collections::HashMap;

fn contains_any(value: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| value.contains(term))
}

pub fn classify_product(title: &str) -> Option<&'static str> {
    let value = title.to_lowercase().replace([' ', '-', '_'], "");
    if value.contains("k12") {
        return Some("k12");
    }
    if value.contains("bug") {
        return Some("bugteam");
    }

    if contains_any(&value, &["free", "pro5x", "pro20x", "gptgo"])
        || contains_any(
            &value,
            &[
                "教程",
                "接码服务",
                "接码专用",
                "镜像站",
                "扫码对接",
                "二维码",
                "虚拟卡",
                "充值",
                "代充",
                "直充",
                "卡充",
                "卡冲",
                "秒冲",
                "cdk",
                "兑换",
                "订阅",
                "内购",
                "苹果账单",
                "官方卡",
            ],
        )
    {
        return None;
    }

    let account_signal = contains_any(
        &value,
        &[
            "成品",
            "半成品",
            "账号",
            "帐号",
            "帳號",
            "独享号",
            "未接码",
            "已接码",
            "邮箱",
            "icloud",
            "gmail",
            "outlook",
            "rt",
            "json",
            "反代",
            "网页",
            "首登",
            "一卡一绑",
            "绑卡",
            "日抛",
            "周抛",
            "手搓",
        ],
    );
    if !account_signal {
        let tool_compatibility = value.contains("sub2api") && value.contains("cpa");
        let team_signal = value.contains("team")
            && (tool_compatibility
                || (value.contains("速刷")
                    && contains_any(&value, &["刀", "美元", "美金", "usd", "$"])));
        return team_signal.then_some("bugteam");
    }

    if value.contains("plus") || value.contains("puls") {
        return Some("gptplus");
    }
    let has_gpt = value.contains("gpt") || value.contains("chatgpt");
    let implicit_account = contains_any(&value, &["成品", "账号", "帐号", "帳號", "独享号"]);
    let implicit_plus = contains_any(
        &value,
        &[
            "upi",
            "u渠道",
            "印度",
            "阿三",
            "越南",
            "韩国",
            "菲区",
            "渠道",
            "一卡一指纹",
            "质保",
        ],
    );
    (has_gpt && implicit_account && implicit_plus).then_some("gptplus")
}

#[cfg(test)]
mod tests {
    use super::classify_product;

    #[test]
    fn classifies_tool_compatible_weekly_team_as_bugteam() {
        assert_eq!(
            classify_product(
                "【8月1日-17点50新车】周限 team，速刷号无任何质保，仅支持sub2api和cpa"
            ),
            Some("bugteam")
        );
    }
}

pub fn match_terms(title: &str) -> Vec<String> {
    match classify_product(title) {
        Some("k12") => vec!["K12".to_string()],
        Some("gptplus") => vec!["PLUS".to_string()],
        Some("bugteam") if title.to_lowercase().contains("team") => {
            vec!["TEAM".to_string(), "BUG".to_string()]
        }
        Some("bugteam") => vec!["BUG".to_string()],
        _ => Vec::new(),
    }
}

pub fn verification_status(title: &str, description: &str, tags: &[String]) -> String {
    let primary = title.to_lowercase();
    let secondary = format!(
        "{} {}",
        description.to_lowercase(),
        tags.join(" ").to_lowercase()
    );
    for value in [&primary, &secondary] {
        if contains_any(
            value,
            &[
                "未接码",
                "未接过码",
                "没有接码",
                "无接码",
                "需要接码",
                "需接码",
                "未绑手机",
                "未绑定手机号",
                "仅网页",
                "仅web",
                "只能网页",
                "网页端专用",
                "web专用",
            ],
        ) {
            return "unverified".to_string();
        }
        if contains_any(
            value,
            &[
                "已接码",
                "接过码",
                "接码成品",
                "接码号",
                "已绑手机",
                "已绑定手机号",
                "带手机号",
                "codex已接码",
            ],
        ) {
            return "verified".to_string();
        }
    }
    "unknown".to_string()
}

fn quantile(values: &mut [f64], ratio: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    values[((values.len() - 1) as f64 * ratio).floor() as usize]
}

fn money(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[derive(Clone, Debug)]
pub struct PriceProfile {
    pub median: f64,
    pub weighted_average: f64,
    pub bargain: f64,
    pub affordable_ceiling: f64,
    pub confidence: &'static str,
}

pub fn price_profiles(products: &[MarketProduct]) -> HashMap<String, PriceProfile> {
    let mut groups: HashMap<String, Vec<(f64, i64)>> = HashMap::new();
    for product in products {
        if let Some(category) = &product.category {
            if product.total_price > 0.0 && product.stock_count > 0 {
                groups
                    .entry(category.clone())
                    .or_default()
                    .push((product.total_price, product.stock_count));
            }
        }
    }

    let mut output = HashMap::new();
    for (category, rows) in groups {
        let mut prices = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let median = quantile(&mut prices, 0.5);
        let mut deviations = prices
            .iter()
            .map(|price| (price - median).abs())
            .collect::<Vec<_>>();
        let mad = quantile(&mut deviations, 0.5);
        let ceiling = if rows.len() < 4 {
            f64::INFINITY
        } else {
            money((median * 1.15).max(median + 3.0 * 1.4826 * mad))
        };
        let affordable = rows
            .iter()
            .filter(|row| row.0 <= ceiling)
            .copied()
            .collect::<Vec<_>>();
        let mut stocks = affordable
            .iter()
            .map(|row| row.1 as f64)
            .collect::<Vec<_>>();
        let stock_cap = quantile(&mut stocks, 0.75).max(1.0);
        let weighted_stock = affordable
            .iter()
            .map(|row| (row.1 as f64).min(stock_cap))
            .sum::<f64>();
        let weighted_average = if weighted_stock > 0.0 {
            money(
                affordable
                    .iter()
                    .map(|row| row.0 * (row.1 as f64).min(stock_cap))
                    .sum::<f64>()
                    / weighted_stock,
            )
        } else {
            0.0
        };
        let mut affordable_prices = affordable.iter().map(|row| row.0).collect::<Vec<_>>();
        output.insert(
            category,
            PriceProfile {
                median: money(median),
                weighted_average,
                bargain: money(quantile(&mut affordable_prices, 0.25)),
                affordable_ceiling: ceiling,
                confidence: if rows.len() >= 12 {
                    "high"
                } else if rows.len() >= 4 {
                    "medium"
                } else {
                    "low"
                },
            },
        );
    }
    output
}

pub fn category_metrics(products: &[MarketProduct]) -> Vec<MarketCategoryMetric> {
    let profiles = price_profiles(products);
    let labels = [
        ("k12", "K12"),
        ("gptplus", "GPT Plus"),
        ("bugteam", "BUG TEAM"),
    ];
    labels
        .iter()
        .map(|(key, label)| {
            let rows = products
                .iter()
                .filter(|product| product.category.as_deref() == Some(*key))
                .collect::<Vec<_>>();
            let profile = profiles.get(*key);
            MarketCategoryMetric {
                key: (*key).to_string(),
                label: (*label).to_string(),
                total_stock: rows.iter().map(|product| product.stock_count).sum(),
                weighted_average_price: profile
                    .map(|item| item.weighted_average)
                    .unwrap_or_default(),
                minimum_price: rows
                    .iter()
                    .map(|product| product.total_price)
                    .filter(|price| *price > 0.0)
                    .min_by(|a, b| a.total_cmp(b))
                    .unwrap_or_default(),
                product_count: rows.len() as i64,
            }
        })
        .collect()
}
