use chrono::{DateTime, Utc};
use rusqlite::{Connection, Result};

pub struct ThreatRecord {
    pub ip: String,
    pub country: String,
    pub connections: u32,
    pub ports: String,
    pub level: String,
    pub timestamp: DateTime<Utc>,
}

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS network_threats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ip TEXT NOT NULL,
            country TEXT NOT NULL,
            connections INTEGER NOT NULL,
            ports TEXT NOT NULL,
            level TEXT NOT NULL,
            timestamp TEXT NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_ip_timestamp ON network_threats (ip, timestamp DESC)",
        [],
    )?;

    init_ip_stats(&conn)?;
    init_whitelist(&conn)?;

    Ok(conn)
}

pub fn insert_threat(conn: &Connection, threat: &ThreatRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO network_threats (ip, country, connections, ports, level, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        [
            &threat.ip,
            &threat.country,
            &threat.connections.to_string(),
            &threat.ports,
            &threat.level,
            &threat.timestamp.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_latest_threats(conn: &Connection, limit_per_ip: usize) -> Result<Vec<ThreatRecord>> {
    let mut stmt = conn.prepare(
        "SELECT ip, country, connections, ports, level, timestamp
         FROM network_threats
         WHERE id IN (
             SELECT id FROM (
                 SELECT id, 
                        ROW_NUMBER() OVER (PARTITION BY ip ORDER BY timestamp DESC) as rn
                 FROM network_threats
             ) WHERE rn <= ?1
         )
         ORDER BY timestamp DESC, 
                  CASE level 
                      WHEN 'CRITICAL' THEN 1 
                      WHEN 'WARNING' THEN 2 
                      ELSE 3 
                  END",
    )?;

    let threat_rows = stmt.query_map([limit_per_ip as i32], |row| {
        Ok(ThreatRecord {
            ip: row.get(0)?,
            country: row.get(1)?,
            connections: row.get(2)?,
            ports: row.get(3)?,
            level: row.get(4)?,
            timestamp: row
                .get::<_, String>(5)?
                .parse()
                .unwrap_or_else(|_| Utc::now()),
        })
    })?;

    let mut threats: Vec<ThreatRecord> = threat_rows.filter_map(|r| r.ok()).collect();

    threats.sort_by(|a, b| {
        let level_order = |l: &str| match l {
            "CRITICAL" => 0,
            "WARNING" => 1,
            _ => 2,
        };
        level_order(&a.level)
            .cmp(&level_order(&b.level))
            .then_with(|| b.timestamp.cmp(&a.timestamp))
    });

    Ok(threats)
}

pub fn clear_threats(conn: &Connection) -> Result<usize> {
    let deleted = conn.execute("DELETE FROM network_threats", [])?;
    Ok(deleted)
}

pub fn init_whitelist(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS admin_whitelist (
            ip TEXT PRIMARY KEY,
            added_at TEXT NOT NULL
        )",
        [],
    )?;
    Ok(())
}

pub fn add_to_whitelist(conn: &Connection, ip: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO admin_whitelist (ip, added_at) VALUES (?1, ?2)",
        [ip, &chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn get_whitelist(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT ip FROM admin_whitelist")?;
    let ips = stmt.query_map([], |row| row.get(0))?;
    Ok(ips.filter_map(|r| r.ok()).collect())
}

pub fn init_ip_stats(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ip_stats (
            ip TEXT PRIMARY KEY,
            country TEXT,
            total_connections INTEGER DEFAULT 0,
            ports TEXT,
            first_seen TEXT,
            last_seen TEXT,
            level TEXT,
            methods TEXT,
            urls TEXT,
            last_updated TEXT
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_ip_stats_total ON ip_stats (total_connections DESC)",
        [],
    )?;

    Ok(())
}

pub fn upsert_ip_stat(
    conn: &Connection,
    ip: &str,
    country: &str,
    connections: i64,
    ports: &str,
    level: &str,
    methods: &[String],
    urls: &[String],
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let methods_str = methods.join(",");
    let urls_str = urls.join(",");

    conn.execute(
        "INSERT OR REPLACE INTO ip_stats 
         (ip, country, total_connections, ports, first_seen, last_seen, level, methods, urls, last_updated)
         VALUES (
             ?1, ?2, 
             COALESCE((SELECT total_connections FROM ip_stats WHERE ip = ?1), 0) + ?3,
             ?4,
             COALESCE((SELECT first_seen FROM ip_stats WHERE ip = ?1), ?5),
             ?5,
             ?6,
             ?7,
             ?8,
             ?9
         )",
        [
            ip,
            country,
            &connections.to_string(),
            ports,
            &now,
            level,
            &methods_str,
            &urls_str,
            &now,
        ],
    )?;

    Ok(())
}

pub fn get_top_attackers(conn: &Connection, limit: usize) -> Result<Vec<serde_json::Value>> {
    let whitelist = get_whitelist(conn)?;
    let whitelist_str = whitelist
        .iter()
        .map(|ip| format!("'{}'", ip))
        .collect::<Vec<_>>()
        .join(",");

    let where_clause = if whitelist_str.is_empty() {
        "".to_string()
    } else {
        format!("WHERE ip NOT IN ({})", whitelist_str)
    };

    let query = format!(
        "SELECT ip, country, total_connections, ports, first_seen, last_seen, level
         FROM ip_stats
         {}
         ORDER BY total_connections DESC
         LIMIT ?1",
        where_clause
    );

    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([limit as i32], |row| {
        Ok(serde_json::json!({
            "ip": row.get::<_, String>(0)?,
            "country": row.get::<_, String>(1)?,
            "total_connections": row.get::<_, i64>(2)?,
            "ports": row.get::<_, String>(3)?,
            "first_seen": row.get::<_, String>(4)?,
            "last_seen": row.get::<_, String>(5)?,
            "level": row.get::<_, String>(6)?
        }))
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn clear_ip(conn: &Connection, ip: &str) -> Result<()> {
    conn.execute("DELETE FROM ip_stats WHERE ip = ?1", [ip])?;
    conn.execute("DELETE FROM network_threats WHERE ip = ?1", [ip])?;
    Ok(())
}
