use super::*;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

#[test]
fn stores_and_updates_account_priority() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let (account, action) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "sk-test-priority".to_string(),
            priority: Some(7),
            ..NewAccount::default()
        })
        .unwrap();
    assert_eq!(action, UpsertAction::Created);
    assert_eq!(account.priority, 7);

    assert!(db.set_priority(&account.id, 2).unwrap());
    assert_eq!(db.get_account(&account.id).unwrap().unwrap().priority, 2);
}

#[test]
fn edits_existing_relay_and_keeps_secret_when_omitted() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let (account, _) = db
        .upsert_account(&NewAccount {
            name: "Before".to_string(),
            account_type: "api_key".to_string(),
            api_key: "sk-original".to_string(),
            base_url: "https://relay.example/v1".to_string(),
            models: Some(vec!["gpt-5".to_string()]),
            ..NewAccount::default()
        })
        .unwrap();

    let updated = db
        .update_account(
            &account.id,
            &AccountUpdate {
                name: "After".to_string(),
                api_key: None,
                base_url: "https://new-api.example.com/".to_string(),
                models: vec!["gpt-5.6".to_string()],
                priority: 3,
                weight: 4,
                concurrency: 12,
                rate_multiplier: 1.25,
            },
        )
        .unwrap()
        .unwrap();

    assert_eq!(updated.name, "After");
    assert_eq!(updated.api_key, "sk-original");
    assert_eq!(updated.base_url, "https://new-api.example.com");
    assert_eq!(updated.models, ["gpt-5.6"]);
    assert_eq!(updated.priority, 3);
    assert_eq!(updated.weight, 4);
    assert_eq!(updated.concurrency, 12);
    assert_eq!(updated.rate_multiplier, 1.25);
}

#[test]
fn reimporting_trashed_account_restores_it() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let imported = NewAccount {
        name: "Restored account".to_string(),
        account_type: "oauth".to_string(),
        access_token: "access-restored".to_string(),
        refresh_token: "refresh-restored".to_string(),
        email: "restored@example.com".to_string(),
        ..NewAccount::default()
    };
    let (account, action) = db.upsert_account(&imported).unwrap();
    assert_eq!(action, UpsertAction::Created);
    assert!(db.delete_account(&account.id).unwrap());
    assert!(db.list_accounts().unwrap().is_empty());

    let (restored, action) = db.upsert_account(&imported).unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(restored.id, account.id);
    assert_eq!(restored.status, "active");
    assert_eq!(db.list_accounts().unwrap().len(), 1);
    assert!(db.list_trashed_accounts().unwrap().is_empty());
}

#[test]
fn merges_one_quota_cache_entry_without_losing_complete_usage_fields() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    db.set_setting(
        "aether:quota_cache",
        r#"{"account-a":{"quota":{"plan_type":"plus","additional_rate_limits":[{"limit_name":"Codex"}],"rate_limit":{"primary_window":{"num_requests":42,"used_percent":10}}},"cached_at":1000},"account-b":{"quota":{"plan_type":"team"},"cached_at":1000}}"#,
    )
    .unwrap();

    db.merge_json_setting_entries(
        "aether:quota_cache",
        &[(
            "account-a".to_string(),
            r#"{"quota":{"rate_limit":{"primary_window":{"used_percent":25,"remaining_percent":75}},"fetched_at":2000},"cached_at":2000000}"#.to_string(),
        )],
    )
    .unwrap();

    let value: serde_json::Value =
        serde_json::from_str(&db.get_setting("aether:quota_cache").unwrap().unwrap()).unwrap();
    assert_eq!(
        value.pointer("/account-a/quota/rate_limit/primary_window/used_percent"),
        Some(&json!(25))
    );
    assert_eq!(
        value.pointer("/account-a/quota/rate_limit/primary_window/num_requests"),
        Some(&json!(42))
    );
    assert_eq!(
        value.pointer("/account-a/quota/additional_rate_limits/0/limit_name"),
        Some(&json!("Codex"))
    );
    assert_eq!(
        value.pointer("/account-b/quota/plan_type"),
        Some(&json!("team"))
    );
}

#[test]
fn stores_exports_and_preserves_routing_configuration() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let (account, action) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "sk-test-routing".to_string(),
            models: Some(vec![
                " gpt-5 ".to_string(),
                "gpt-5".to_string(),
                "gpt-5-mini".to_string(),
            ]),
            weight: Some(4),
            concurrency: Some(12),
            ..NewAccount::default()
        })
        .unwrap();
    assert_eq!(action, UpsertAction::Created);
    assert_eq!(account.models, ["gpt-5", "gpt-5-mini"]);
    assert_eq!(account.weight, 4);
    assert_eq!(account.concurrency, 12);

    let (preserved, action) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "sk-test-routing".to_string(),
            ..NewAccount::default()
        })
        .unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(preserved.models, account.models);
    assert_eq!(preserved.weight, 4);
    assert_eq!(preserved.concurrency, 12);

    let (updated, _) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "sk-test-routing".to_string(),
            models: Some(Vec::new()),
            weight: Some(2),
            ..NewAccount::default()
        })
        .unwrap();
    assert!(updated.models.is_empty());
    assert_eq!(updated.weight, 2);

    let exported = db.export_data().unwrap();
    assert_eq!(exported.pointer("/accounts/0/models"), Some(&json!([])));
    assert_eq!(exported.pointer("/accounts/0/weight"), Some(&json!(2)));
    assert_eq!(
        exported.pointer("/accounts/0/concurrency"),
        Some(&json!(12))
    );
    assert_eq!(exported.pointer("/accounts/0/type"), Some(&json!("apikey")));
    assert_eq!(
        exported.pointer("/accounts/0/credentials/api_key"),
        Some(&json!("sk-test-routing"))
    );
}

#[test]
fn api_key_identity_includes_base_url_and_weight_is_bounded() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let (first, _) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "shared-key".to_string(),
            base_url: "https://relay-a.example/v1/".to_string(),
            weight: Some(1),
            ..NewAccount::default()
        })
        .unwrap();
    let (second, action) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "shared-key".to_string(),
            base_url: "https://relay-b.example/v1".to_string(),
            weight: Some(1000),
            ..NewAccount::default()
        })
        .unwrap();
    assert_eq!(action, UpsertAction::Created);
    assert_ne!(first.id, second.id);

    let (same_first, action) = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "shared-key".to_string(),
            base_url: "https://relay-a.example/v1".to_string(),
            ..NewAccount::default()
        })
        .unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(same_first.id, first.id);

    let error = db
        .upsert_account(&NewAccount {
            account_type: "api_key".to_string(),
            api_key: "invalid-weight".to_string(),
            weight: Some(1001),
            ..NewAccount::default()
        })
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("weight must be between 1 and 1000"));
}

#[test]
fn oauth_identity_does_not_collapse_users_in_the_same_chatgpt_account() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let shared_account_id = "shared-workspace";
    let (first, _) = db
        .upsert_account(&NewAccount {
            account_type: "oauth".to_string(),
            refresh_token: "refresh-first".to_string(),
            chatgpt_account_id: shared_account_id.to_string(),
            chatgpt_user_id: "user-first".to_string(),
            email: "first@example.com".to_string(),
            ..NewAccount::default()
        })
        .unwrap();
    let (second, action) = db
        .upsert_account(&NewAccount {
            account_type: "oauth".to_string(),
            refresh_token: "refresh-second".to_string(),
            chatgpt_account_id: shared_account_id.to_string(),
            chatgpt_user_id: "user-second".to_string(),
            email: "second@example.com".to_string(),
            ..NewAccount::default()
        })
        .unwrap();

    assert_eq!(action, UpsertAction::Created);
    assert_ne!(first.id, second.id);
    assert_eq!(db.list_accounts().unwrap().len(), 2);

    let (same_first, action) = db
        .upsert_account(&NewAccount {
            name: "first updated".to_string(),
            account_type: "oauth".to_string(),
            refresh_token: "rotated-refresh-first".to_string(),
            chatgpt_account_id: shared_account_id.to_string(),
            chatgpt_user_id: "user-first".to_string(),
            email: "first@example.com".to_string(),
            ..NewAccount::default()
        })
        .unwrap();
    assert_eq!(action, UpsertAction::Updated);
    assert_eq!(same_first.id, first.id);
    assert_eq!(db.list_accounts().unwrap().len(), 2);
}

#[test]
fn migrates_legacy_account_table_with_routing_defaults() {
    let path = std::env::temp_dir().join(format!(
        "sub2api-db-migration-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                base_url TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             );
             INSERT INTO accounts (id, name, api_key)
             VALUES ('legacy', 'Legacy API Key', 'sk-legacy');",
        )
        .unwrap();
    }

    let db = Db::new(&path).unwrap();
    let account = db.get_account("legacy").unwrap().unwrap();
    assert!(account.models.is_empty());
    assert_eq!(account.weight, 1);
    assert_eq!(account.api_key, "sk-legacy");
    drop(db);
    let _ = std::fs::remove_file(path);
}

fn log_start(account: &Account, request_id: &str, attempt_index: i64) -> RequestLogStart {
    RequestLogStart {
        request_id: request_id.to_string(),
        attempt_index,
        account_id: Some(account.id.clone()),
        account_name: account.name.clone(),
        account_type: account.account_type.clone(),
        source: "proxy".to_string(),
        method: "POST".to_string(),
        path: "/v1/responses?secret=discarded".to_string(),
        endpoint_family: "responses".to_string(),
        model: "gpt-5".to_string(),
        streaming: true,
    }
}

#[test]
fn request_logs_are_sanitized_and_aggregated_for_monitoring() {
    let db = Db::new(Path::new(":memory:")).unwrap();
    let (account, _) = db
        .upsert_account(&NewAccount {
            name: "Monitor account".to_string(),
            account_type: "api_key".to_string(),
            api_key: "monitor-key".to_string(),
            concurrency: Some(8),
            ..NewAccount::default()
        })
        .unwrap();

    let success = db
        .insert_request_log(&log_start(&account, "request-1", 0))
        .unwrap();
    db.mark_request_log_response(success, 200, 900).unwrap();
    db.complete_request_log(
        success,
        "success",
        Some(200),
        Some(900),
        1_200,
        RequestLogUsage {
            total_tokens: 150,
            estimated_cost: 0.001,
            ..RequestLogUsage::default()
        },
        "Bearer secret-token\ncompleted",
    )
    .unwrap();

    let slow = db
        .insert_request_log(&log_start(&account, "request-2", 0))
        .unwrap();
    db.complete_request_log(
        slow,
        "success",
        Some(200),
        Some(6_000),
        6_500,
        RequestLogUsage::default(),
        "",
    )
    .unwrap();

    let retry = db
        .insert_request_log(&log_start(&account, "request-3", 1))
        .unwrap();
    db.complete_request_log(
        retry,
        "retry",
        Some(429),
        Some(300),
        400,
        RequestLogUsage::default(),
        "rate limited",
    )
    .unwrap();

    let page = db.list_request_logs(RequestLogQuery::default()).unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.items[0].path, "/v1/responses");
    assert!(!page.items[2].message.contains("secret-token"));

    let overview = db.request_log_overview().unwrap();
    assert_eq!(overview.total_requests, 3);
    assert_eq!(overview.total_attempts, 3);
    assert_eq!(overview.success_attempts, 2);
    assert_eq!(overview.retry_attempts, 1);

    let first_page = db
        .list_request_logs(RequestLogQuery {
            limit: Some(2),
            ..RequestLogQuery::default()
        })
        .unwrap();
    assert!(first_page.has_more);
    let second_page = db
        .list_request_logs(RequestLogQuery {
            before_id: first_page.next_before_id,
            limit: Some(2),
            ..RequestLogQuery::default()
        })
        .unwrap();
    assert_eq!(second_page.items.len(), 1);
    assert!(!second_page.has_more);

    let snapshot = db.channel_monitor_snapshot().unwrap();
    assert_eq!(snapshot.len(), 1);
    let item = &snapshot[0];
    assert_eq!(item.available_24h, 2);
    assert_eq!(item.failed_24h, 1);
    assert_eq!(item.attempts_24h, 3);
    assert!((item.availability_24h.unwrap() - 200.0 / 3.0).abs() < 1e-9);
    assert_eq!(item.timeline.len(), 3);
    assert_eq!(item.timeline[0].status, "error");
    assert_eq!(item.timeline[1].status, "degraded");
    assert_eq!(item.timeline[2].source, "traffic");
    assert_eq!(db.clear_request_logs().unwrap(), 3);
}

#[test]
fn reopening_database_closes_pending_request_logs() {
    let path = std::env::temp_dir().join(format!(
        "sub2api-request-log-recovery-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    {
        let db = Db::new(&path).unwrap();
        let (account, _) = db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "pending-log-key".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        db.insert_request_log(&log_start(&account, "pending-request", 0))
            .unwrap();
    }

    let db = Db::new(&path).unwrap();
    let page = db.list_request_logs(RequestLogQuery::default()).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].status, "cancelled");
    assert!(page.items[0].completed_at.is_some());
    drop(db);
    let _ = std::fs::remove_file(path);
}
