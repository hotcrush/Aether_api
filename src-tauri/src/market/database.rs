use super::legacy::load_legacy_market;
use super::types::{
    MarketAlertSettings, MarketAnalyticsPoint, MarketEvent, MarketProduct, MarketProtection,
    MarketShop, MarketShopInput, MarketSnapshot,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_SHOPS: &[(&str, &str)] = &[
    ("harvey", "派大星"),
    ("GUEOI7Z9", "鹰鹰小铺"),
    ("OUJ1HPBV", "chiyu"),
    ("yimengai", "一梦AI"),
    ("Tora", "Tora-雪诺AI代购娘小铺"),
    ("2VWX76A4", "牟利ai"),
    ("911", "金幺の小店"),
    ("P98T49H8", "稳中求胜"),
    ("TH52WUW7", "boji1334yd"),
    ("YA3NLPX6", "AI小店"),
    ("wuku", "老马AI（源头直供，招代理）"),
    ("youzhi", "优质AI科技"),
    ("YUJI", "YUJI"),
    ("ymymai", "亚米整合服务供应商"),
    ("FRNX1ZU8", "追梦AI"),
    ("7HVUEC3Y", "464"),
    ("M18V0XVF", "陆柒科技"),
    ("5KF19IU0", "链动小铺 / 5KF19IU0"),
    ("3GYP7PKO", "直连AI"),
    ("mirage", "幻境MirageAI"),
    ("echo_dream", "AI小铺"),
    ("SubAIP", "AI源头批发旗舰店"),
    ("SJ1BEJAC", "商家8719"),
    ("GU3XQH61", "NiuGe AI 加钟站"),
    ("luoerl", "链动小铺 / luoerl"),
    ("ZPISRC7G", "琪琪科技"),
    ("5OFQXIM1", "冷热lab"),
];

const NEW_DEFAULT_SHOPS_2026_08_12: &[(&str, &str)] = &[
    ("TH52WUW7", "boji1334yd"),
    ("YA3NLPX6", "AI小店"),
    ("YUJI", "YUJI"),
    ("FRNX1ZU8", "追梦AI"),
    ("7HVUEC3Y", "464"),
    ("M18V0XVF", "陆柒科技"),
    ("5KF19IU0", "链动小铺 / 5KF19IU0"),
    ("3GYP7PKO", "直连AI"),
    ("SJ1BEJAC", "商家8719"),
    ("GU3XQH61", "NiuGe AI 加钟站"),
    ("ZPISRC7G", "琪琪科技"),
    ("5OFQXIM1", "冷热lab"),
];

const DEFAULT_SHOPS_MIGRATION: &str = "default_shops_2026_08_12";

#[derive(Debug)]
pub struct MarketDatabase {
    path: PathBuf,
}

impl MarketDatabase {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let database = Self { path };
        let mut conn = database.connection()?;
        database.initialize(&mut conn)?;
        database.seed_default_shops(&mut conn)?;
        Ok(database)
    }

    fn connection(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(|error| error.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )
        .map_err(|error| error.to_string())?;
        Ok(conn)
    }

    fn initialize(&self, conn: &mut Connection) -> Result<(), String> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS market_shops (
                token TEXT PRIMARY KEY,
                platform TEXT NOT NULL DEFAULT 'liandx',
                fallback_name TEXT NOT NULL,
                name TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                display_order INTEGER NOT NULL DEFAULT 0,
                ok INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                failure_count INTEGER NOT NULL DEFAULT 0,
                blocked_until TEXT,
                fee_rate REAL NOT NULL DEFAULT 0,
                fee_payer INTEGER NOT NULL DEFAULT 0,
                fee_checked_at TEXT,
                profile_checked_at TEXT,
                last_checked_at TEXT,
                last_success_at TEXT,
                goods_types TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS market_products (
                id TEXT PRIMARY KEY,
                goods_key TEXT NOT NULL,
                shop_token TEXT NOT NULL REFERENCES market_shops(token) ON DELETE CASCADE,
                shop_name TEXT NOT NULL,
                shop_url TEXT NOT NULL,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                price REAL NOT NULL,
                fee REAL NOT NULL DEFAULT 0,
                fee_rate REAL NOT NULL DEFAULT 0,
                fee_payer INTEGER NOT NULL DEFAULT 0,
                total_price REAL NOT NULL,
                market_price REAL NOT NULL DEFAULT 0,
                stock_count INTEGER NOT NULL DEFAULT 0,
                source_category TEXT NOT NULL DEFAULT '',
                category TEXT,
                match_terms TEXT NOT NULL DEFAULT '[]',
                verification_status TEXT NOT NULL DEFAULT 'unknown',
                missing_count INTEGER NOT NULL DEFAULT 0,
                first_seen_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_market_products_shop ON market_products(shop_token);
            CREATE INDEX IF NOT EXISTS idx_market_products_category ON market_products(category, total_price);
            CREATE TABLE IF NOT EXISTS market_runtime (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS market_analytics_points (
                captured_at TEXT PRIMARY KEY,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_market_analytics_time ON market_analytics_points(captured_at);
            CREATE TABLE IF NOT EXISTS market_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                expires_at TEXT,
                severity TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                section TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                read_at TEXT,
                notified_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_market_events_time ON market_events(occurred_at DESC);
            CREATE INDEX IF NOT EXISTS idx_market_events_unread ON market_events(read_at, seq DESC);
            CREATE TABLE IF NOT EXISTS market_notification_deliveries (
                event_id TEXT NOT NULL REFERENCES market_events(event_id) ON DELETE CASCADE,
                channel TEXT NOT NULL,
                status TEXT NOT NULL,
                attempted_at TEXT NOT NULL,
                error TEXT,
                PRIMARY KEY(event_id, channel)
            );",
        )
        .map_err(|error| error.to_string())
    }

    fn seed_default_shops(&self, conn: &mut Connection) -> Result<(), String> {
        let migrated: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_runtime WHERE key=?1)",
                [DEFAULT_SHOPS_MIGRATION],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if migrated {
            return Ok(());
        }
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let existing_shop_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM market_shops", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let shops = if existing_shop_count == 0 {
            DEFAULT_SHOPS
        } else {
            NEW_DEFAULT_SHOPS_2026_08_12
        };
        let display_order: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(display_order), -1) + 1 FROM market_shops",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        for (index, (token, name)) in shops.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO market_shops(token, platform, fallback_name, name, enabled, display_order)
                 VALUES(?1, 'liandx', ?2, ?2, 1, ?3)",
                params![token, name, display_order + index as i64],
            )
            .map_err(|error| error.to_string())?;
        }
        set_runtime_tx(&tx, DEFAULT_SHOPS_MIGRATION, &true)?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub fn import_legacy(&self, data_dir: &Path) -> Result<bool, String> {
        let Some(import) = load_legacy_market(data_dir)? else {
            return Ok(false);
        };
        let mut conn = self.connection()?;
        let already_imported: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_runtime WHERE key='legacy_import_v1')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if already_imported {
            return Ok(false);
        }

        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let product_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM market_products", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if product_count == 0 && !import.products.is_empty() {
            for (index, shop) in import.shops.iter().enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO market_shops(token, platform, fallback_name, name, enabled, display_order)
                     VALUES(?1, 'liandx', ?2, ?3, 1, ?4)",
                    params![shop.token, shop.fallback_name, shop.name, index as i64],
                )
                .map_err(|error| error.to_string())?;
                persist_shop(&tx, shop)?;
            }
            for product in &import.products {
                tx.execute(
                    "INSERT OR IGNORE INTO market_shops(token, platform, fallback_name, name, enabled, display_order)
                     VALUES(?1, 'liandx', ?2, ?2, 1, 1000)",
                    params![product.shop_token, product.shop_name],
                )
                .map_err(|error| error.to_string())?;
                persist_product(&tx, product)?;
            }
            set_runtime_tx(&tx, "protection", &import.protection)?;
            set_runtime_tx(&tx, "last_checked_at", &import.last_checked_at)?;
            set_runtime_tx(&tx, "next_refresh_at", &import.next_refresh_at)?;
        }

        for point in &import.points {
            tx.execute(
                "INSERT OR IGNORE INTO market_analytics_points(captured_at, payload) VALUES(?1, ?2)",
                params![
                    point.captured_at,
                    serde_json::to_string(point).map_err(|error| error.to_string())?
                ],
            )
            .map_err(|error| error.to_string())?;
        }
        for event in &import.events {
            tx.execute(
                "INSERT OR IGNORE INTO market_events(event_id, kind, entity_type, entity_id,
                 occurred_at, expires_at, severity, title, body, section, payload, read_at, notified_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    event.event_id,
                    event.kind,
                    event.entity_type,
                    event.entity_id,
                    event.occurred_at,
                    event.expires_at,
                    event.severity,
                    event.title,
                    event.body,
                    event.section,
                    event.payload.to_string(),
                    event.read_at,
                    event.notified_at,
                ],
            )
            .map_err(|error| error.to_string())?;
        }
        set_runtime_tx(&tx, "legacy_import_v1", &chrono::Utc::now().to_rfc3339())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn load_snapshot(&self) -> Result<MarketSnapshot, String> {
        let conn = self.connection()?;
        let mut shops = self.load_shops_from(&conn)?;
        let products = self.load_products_from(&conn)?;
        apply_shop_counts(&mut shops, &products);
        let protection = self
            .get_runtime_from::<MarketProtection>(&conn, "protection")?
            .unwrap_or_else(|| MarketSnapshot::default().protection);
        let last_checked_at = self
            .get_runtime_from::<Option<String>>(&conn, "last_checked_at")?
            .flatten();
        let next_refresh_at = self
            .get_runtime_from::<Option<String>>(&conn, "next_refresh_at")?
            .flatten();
        let unread_alert_count = conn
            .query_row(
                "SELECT COUNT(*) FROM market_events WHERE read_at IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let enabled = shops.iter().filter(|shop| shop.enabled).collect::<Vec<_>>();
        let status = if enabled.is_empty() {
            "idle"
        } else if enabled.iter().all(|shop| shop.ok) {
            "online"
        } else if enabled.iter().any(|shop| shop.ok) {
            "partial"
        } else if last_checked_at.is_some() {
            "error"
        } else {
            "loading"
        };
        Ok(MarketSnapshot {
            status: status.to_string(),
            products,
            shops,
            protection,
            last_checked_at,
            next_refresh_at,
            unread_alert_count,
        })
    }

    fn load_shops_from(&self, conn: &Connection) -> Result<Vec<MarketShop>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT platform, token, fallback_name, name, enabled, ok, error, failure_count,
                        blocked_until, fee_rate, fee_payer, fee_checked_at, profile_checked_at,
                        last_checked_at, last_success_at, goods_types
                   FROM market_shops ORDER BY display_order, name COLLATE NOCASE",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let goods_types: String = row.get(15)?;
                Ok(MarketShop {
                    platform: row.get(0)?,
                    token: row.get(1)?,
                    fallback_name: row.get(2)?,
                    name: row.get(3)?,
                    enabled: row.get::<_, i64>(4)? != 0,
                    ok: row.get::<_, i64>(5)? != 0,
                    error: row.get(6)?,
                    failure_count: row.get(7)?,
                    blocked_until: row.get(8)?,
                    fee_rate: row.get(9)?,
                    fee_payer: row.get(10)?,
                    fee_checked_at: row.get(11)?,
                    profile_checked_at: row.get(12)?,
                    last_checked_at: row.get(13)?,
                    last_success_at: row.get(14)?,
                    goods_types: serde_json::from_str(&goods_types).unwrap_or_default(),
                    product_count: 0,
                    total_stock: 0,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn load_products_from(&self, conn: &Connection) -> Result<Vec<MarketProduct>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT id, goods_key, shop_token, shop_name, shop_url, name, url, price, fee,
                        fee_rate, fee_payer, total_price, market_price, stock_count, source_category,
                        category, match_terms, verification_status, missing_count, first_seen_at, last_seen_at
                   FROM market_products ORDER BY total_price, shop_name COLLATE NOCASE",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let match_terms: String = row.get(16)?;
                Ok(MarketProduct {
                    id: row.get(0)?,
                    goods_key: row.get(1)?,
                    shop_token: row.get(2)?,
                    shop_name: row.get(3)?,
                    shop_url: row.get(4)?,
                    name: row.get(5)?,
                    url: row.get(6)?,
                    price: row.get(7)?,
                    fee: row.get(8)?,
                    fee_rate: row.get(9)?,
                    fee_payer: row.get(10)?,
                    total_price: row.get(11)?,
                    market_price: row.get(12)?,
                    stock_count: row.get(13)?,
                    source_category: row.get(14)?,
                    category: row.get(15)?,
                    match_terms: serde_json::from_str(&match_terms).unwrap_or_default(),
                    verification_status: row.get(17)?,
                    missing_count: row.get(18)?,
                    first_seen_at: row.get(19)?,
                    last_seen_at: row.get(20)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn persist_refresh(
        &self,
        snapshot: &MarketSnapshot,
        point: Option<&MarketAnalyticsPoint>,
        events: &[MarketEvent],
    ) -> Result<Vec<MarketEvent>, String> {
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        for shop in &snapshot.shops {
            persist_shop(&tx, shop)?;
        }
        tx.execute("DELETE FROM market_products", [])
            .map_err(|error| error.to_string())?;
        for product in &snapshot.products {
            persist_product(&tx, product)?;
        }
        set_runtime_tx(&tx, "protection", &snapshot.protection)?;
        set_runtime_tx(&tx, "last_checked_at", &snapshot.last_checked_at)?;
        set_runtime_tx(&tx, "next_refresh_at", &snapshot.next_refresh_at)?;
        if let Some(point) = point {
            tx.execute(
                "INSERT OR REPLACE INTO market_analytics_points(captured_at, payload) VALUES(?1, ?2)",
                params![point.captured_at, serde_json::to_string(point).map_err(|error| error.to_string())?],
            )
            .map_err(|error| error.to_string())?;
        }

        let mut inserted = Vec::new();
        for event in events {
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO market_events(event_id, kind, entity_type, entity_id,
                     occurred_at, expires_at, severity, title, body, section, payload, read_at, notified_at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        event.event_id,
                        event.kind,
                        event.entity_type,
                        event.entity_id,
                        event.occurred_at,
                        event.expires_at,
                        event.severity,
                        event.title,
                        event.body,
                        event.section,
                        event.payload.to_string(),
                        event.read_at,
                        event.notified_at,
                    ],
                )
                .map_err(|error| error.to_string())?;
            if changed > 0 {
                inserted.push(event.clone());
            }
        }

        tx.execute(
            "DELETE FROM market_analytics_points
              WHERE captured_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-31 days')",
            [],
        )
        .map_err(|error| error.to_string())?;
        tx.execute(
            "DELETE FROM market_events
              WHERE occurred_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-90 days')",
            [],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(inserted)
    }

    pub fn upsert_shop(&self, input: &MarketShopInput) -> Result<MarketShop, String> {
        let token = normalize_token(&input.token)?;
        if input.platform != "liandx" {
            return Err("目前仅支持 liandx 店铺".to_string());
        }
        let name = input.fallback_name.trim();
        if name.is_empty() {
            return Err("店铺名称不能为空".to_string());
        }
        let conn = self.connection()?;
        let order: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(display_order), -1) + 1 FROM market_shops",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO market_shops(token, platform, fallback_name, name, enabled, display_order)
             VALUES(?1, ?2, ?3, ?3, ?4, ?5)
             ON CONFLICT(token) DO UPDATE SET platform=excluded.platform,
                 fallback_name=excluded.fallback_name, enabled=excluded.enabled",
            params![token, input.platform, name, input.enabled as i64, order],
        )
        .map_err(|error| {
            if error.to_string().contains("UNIQUE") {
                "店铺 token 已存在".to_string()
            } else {
                error.to_string()
            }
        })?;
        self.load_shops_from(&conn)?
            .into_iter()
            .find(|shop| shop.token == token)
            .ok_or_else(|| "保存店铺后读取失败".to_string())
    }

    pub fn set_shop_enabled(&self, token: &str, enabled: bool) -> Result<(), String> {
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let changed = tx
            .execute(
                "UPDATE market_shops SET enabled=?2, ok=CASE WHEN ?2=0 THEN 0 ELSE ok END,
                 error=CASE WHEN ?2=0 THEN NULL ELSE error END WHERE token=?1",
                params![token, enabled as i64],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("店铺不存在".to_string());
        }
        if !enabled {
            tx.execute("DELETE FROM market_products WHERE shop_token=?1", [token])
                .map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn delete_shop(&self, token: &str) -> Result<(), String> {
        let conn = self.connection()?;
        let changed = conn
            .execute("DELETE FROM market_shops WHERE token=?1", [token])
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("店铺不存在".to_string());
        }
        Ok(())
    }

    pub fn analytics_points(&self, cutoff: &str) -> Result<Vec<MarketAnalyticsPoint>, String> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare(
                "SELECT payload FROM market_analytics_points WHERE captured_at >= ?1 ORDER BY captured_at",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([cutoff], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        let mut points = Vec::new();
        for row in rows {
            let value = row.map_err(|error| error.to_string())?;
            if let Ok(point) = serde_json::from_str(&value) {
                points.push(point);
            }
        }
        Ok(points)
    }

    pub fn events(&self, cutoff: Option<&str>, limit: usize) -> Result<Vec<MarketEvent>, String> {
        let conn = self.connection()?;
        let sql = if cutoff.is_some() {
            "SELECT seq, event_id, kind, entity_type, entity_id, occurred_at, expires_at, severity,
                    title, body, section, payload, read_at, notified_at
               FROM market_events WHERE occurred_at >= ?1 ORDER BY seq DESC LIMIT ?2"
        } else {
            "SELECT seq, event_id, kind, entity_type, entity_id, occurred_at, expires_at, severity,
                    title, body, section, payload, read_at, notified_at
               FROM market_events ORDER BY seq DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
        let mapper = |row: &rusqlite::Row<'_>| {
            let payload: String = row.get(11)?;
            Ok(MarketEvent {
                seq: row.get(0)?,
                event_id: row.get(1)?,
                kind: row.get(2)?,
                entity_type: row.get(3)?,
                entity_id: row.get(4)?,
                occurred_at: row.get(5)?,
                expires_at: row.get(6)?,
                severity: row.get(7)?,
                title: row.get(8)?,
                body: row.get(9)?,
                section: row.get(10)?,
                payload: serde_json::from_str(&payload).unwrap_or_default(),
                read_at: row.get(12)?,
                notified_at: row.get(13)?,
            })
        };
        let rows = if let Some(cutoff) = cutoff {
            stmt.query_map(params![cutoff, limit as i64], mapper)
        } else {
            stmt.query_map(params![limit as i64], mapper)
        }
        .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn mark_events_read(&self, event_ids: &[String], read_at: &str) -> Result<u64, String> {
        let mut conn = self.connection()?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let changed = if event_ids.is_empty() {
            tx.execute(
                "UPDATE market_events SET read_at=?1 WHERE read_at IS NULL",
                [read_at],
            )
            .map_err(|error| error.to_string())?
        } else {
            let mut changed = 0;
            for id in event_ids {
                changed += tx
                    .execute(
                        "UPDATE market_events SET read_at=?2 WHERE event_id=?1 AND read_at IS NULL",
                        params![id, read_at],
                    )
                    .map_err(|error| error.to_string())?;
            }
            changed
        };
        tx.commit().map_err(|error| error.to_string())?;
        Ok(changed as u64)
    }

    pub fn alert_settings(&self) -> Result<MarketAlertSettings, String> {
        let conn = self.connection()?;
        Ok(self
            .get_runtime_from(&conn, "alert_settings")?
            .unwrap_or_default())
    }

    pub fn set_alert_settings(&self, settings: &MarketAlertSettings) -> Result<(), String> {
        let conn = self.connection()?;
        let value = serde_json::to_string(settings).map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO market_runtime(key, value) VALUES('alert_settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [value],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn checkpoint_protection(&self, protection: &MarketProtection) -> Result<(), String> {
        let conn = self.connection()?;
        let value = serde_json::to_string(protection).map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO market_runtime(key, value) VALUES('protection', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [value],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn mark_notified(&self, event_id: &str, channel: &str, now: &str) -> Result<bool, String> {
        let conn = self.connection()?;
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO market_notification_deliveries(event_id, channel, status, attempted_at)
                 VALUES(?1, ?2, 'sent', ?3)",
                params![event_id, channel, now],
            )
            .map_err(|error| error.to_string())?;
        if changed > 0 {
            conn.execute(
                "UPDATE market_events SET notified_at=?2 WHERE event_id=?1",
                params![event_id, now],
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(changed > 0)
    }

    fn get_runtime_from<T: serde::de::DeserializeOwned>(
        &self,
        conn: &Connection,
        key: &str,
    ) -> Result<Option<T>, String> {
        let value = conn
            .query_row(
                "SELECT value FROM market_runtime WHERE key=?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        value
            .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
            .transpose()
    }
}

fn normalize_token(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let token = if let Ok(url) = reqwest::Url::parse(trimmed) {
        if url.scheme() != "https" || url.host_str() != Some("pay.ldxp.cn") {
            return Err("店铺地址必须是 https://pay.ldxp.cn/shop/...".to_string());
        }
        let mut segments = url.path_segments().into_iter().flatten();
        if segments.next() != Some("shop") {
            return Err("无法从地址识别店铺 token".to_string());
        }
        segments.next().unwrap_or_default().to_string()
    } else {
        trimmed.to_string()
    };
    if token.is_empty()
        || token.len() > 80
        || !token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err("店铺 token 格式无效".to_string());
    }
    Ok(token)
}

fn apply_shop_counts(shops: &mut [MarketShop], products: &[MarketProduct]) {
    for shop in shops {
        let rows = products
            .iter()
            .filter(|product| product.shop_token == shop.token)
            .collect::<Vec<_>>();
        shop.product_count = rows.len() as i64;
        shop.total_stock = rows.iter().map(|product| product.stock_count).sum();
    }
}

fn persist_shop(tx: &Transaction<'_>, shop: &MarketShop) -> Result<(), String> {
    tx.execute(
        "UPDATE market_shops SET platform=?2, fallback_name=?3, name=?4, enabled=?5, ok=?6,
            error=?7, failure_count=?8, blocked_until=?9, fee_rate=?10, fee_payer=?11,
            fee_checked_at=?12, profile_checked_at=?13, last_checked_at=?14,
            last_success_at=?15, goods_types=?16 WHERE token=?1",
        params![
            shop.token,
            shop.platform,
            shop.fallback_name,
            shop.name,
            shop.enabled as i64,
            shop.ok as i64,
            shop.error,
            shop.failure_count,
            shop.blocked_until,
            shop.fee_rate,
            shop.fee_payer,
            shop.fee_checked_at,
            shop.profile_checked_at,
            shop.last_checked_at,
            shop.last_success_at,
            serde_json::to_string(&shop.goods_types).map_err(|error| error.to_string())?,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn persist_product(tx: &Transaction<'_>, product: &MarketProduct) -> Result<(), String> {
    tx.execute(
        "INSERT INTO market_products(id, goods_key, shop_token, shop_name, shop_url, name, url,
         price, fee, fee_rate, fee_payer, total_price, market_price, stock_count, source_category,
         category, match_terms, verification_status, missing_count, first_seen_at, last_seen_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
        params![
            product.id,
            product.goods_key,
            product.shop_token,
            product.shop_name,
            product.shop_url,
            product.name,
            product.url,
            product.price,
            product.fee,
            product.fee_rate,
            product.fee_payer,
            product.total_price,
            product.market_price,
            product.stock_count,
            product.source_category,
            product.category,
            serde_json::to_string(&product.match_terms).map_err(|error| error.to_string())?,
            product.verification_status,
            product.missing_count,
            product.first_seen_at,
            product.last_seen_at,
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn set_runtime_tx<T: serde::Serialize>(
    tx: &Transaction<'_>,
    key: &str,
    value: &T,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO market_runtime(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![
            key,
            serde_json::to_string(value).map_err(|error| error.to_string())?
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}
