//! 多提供商档案（Step A）：一组提供商配置的 JSON 整包，存 SQLite kv 表键 `providers`。
//!
//! # 为什么是 kv JSON 整包而不是关系表（2026-09-05 定案）
//! 提供商档案永远整读整写、从不按字段 `WHERE`/排序——按「要查才成列」铁律，
//! 它就是 JSON 叶子：结构演化（如 B 期能力位）零 schema 迁移、零清库。
//!
//! # 密钥边界
//! 本结构**不存密钥**（结构里没有 key 字段）：密钥按提供商 id 存
//! `config.toml` 的 `[keys]` 表（见 [`Secrets`](super::config::Secrets)），
//! DB 文件可随意备份/导出/传阅。
//!
//! # 删除宪法
//! 条目删除 = 逻辑删（`deleted` 标记，列表灰显可恢复）；「彻底删除」才物理移除
//! 并连带清 `[keys]` 中该 id 的密钥。

use std::{collections::BTreeMap, error::Error, path::Path};

use serde::{Deserialize, Serialize};

use super::config::{Api, Secrets};
use super::provider::Provider;
use crate::db::Db;

/// kv 表中的整包键名。
const KV_KEY: &str = "providers";
/// 旧配置（DB-2 单配置时代）在 kv 表中的键名，仅迁移时读取/删除。
const LEGACY_KV_KEY: &str = "config";

/// 一个提供商档案（无密钥）。`serde(default)` 全开：结构演化读旧包不炸。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderEntry {
    /// 稳定 id（"p1"、"p2"…），`[keys]` 与 active 指针都引用它
    pub id: String,
    /// 厂家类型（通用兼容 / 小米 MiMo），新增菜单由 [`Provider`] 枚举驱动
    pub kind: Provider,
    /// 显示名（列表与详情标题）
    pub name: String,
    /// 基础地址（空 = 用 kind 内置端点；MiMo 建条目时预填官方端点便于查看）
    pub base_url: String,
    /// 路径后缀（可空）：生效地址 = base + "/" + 后缀，如代理网关路径
    pub url_suffix: String,
    /// 接口族（定制提供商由 [`locked_api`](Provider::locked_api) 强制覆盖）
    pub api: Option<Api>,
    /// 是否流式
    pub stream: bool,
    /// 可用模型表：模型 ID → 显示别名（空别名 = 原样显示；B 期升级为带能力的模型对象）
    pub models: BTreeMap<String, String>,
    /// 逻辑删除标记（删除宪法）
    pub deleted: bool,
}

impl Default for ProviderEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: Provider::default(),
            name: String::new(),
            base_url: String::new(),
            url_suffix: String::new(),
            api: None,
            stream: true,
            models: BTreeMap::new(),
            deleted: false,
        }
    }
}

/// 提供商档案整包。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    /// 使用中条目的 id；空 = 无可用提供商（对话侧走配置错误提示）
    pub active: String,
    /// 全局当前模型 ID（切换提供商后可能不在其列表内，发送时原样携带由网关裁决）
    pub active_model: String,
    /// 全部条目（含逻辑删除项）
    pub list: Vec<ProviderEntry>,
}

/// base + 后缀拼接：斜杠规范化，空后缀零影响。
///
/// ```
/// use o_wakaka::ai::providers::join_url;
///
/// assert_eq!(join_url("https://x.com/v1/", ""), "https://x.com/v1");
/// assert_eq!(join_url("https://x.com/v1", "/proxy/aigw"), "https://x.com/v1/proxy/aigw");
/// assert_eq!(join_url("https://x.com/v1/", "/p/"), "https://x.com/v1/p");
/// assert_eq!(join_url("", "p"), "/p");
/// ```
pub fn join_url(base: &str, suffix: &str) -> String {
    let b = base.trim_end_matches('/');
    let s = suffix.trim().trim_matches('/');
    if s.is_empty() {
        b.to_string()
    } else {
        format!("{b}/{s}")
    }
}

/// 别名表 ⇆ 弹窗多行文本（每行 `模型ID=显示名`，无 `=` 则别名为空；空行跳过）。
///
/// 往返一致（B 期换成正式模型管理前的简化位）：
///
/// ```
/// use std::collections::BTreeMap;
/// use o_wakaka::ai::providers::{fmt_aliases, parse_aliases};
///
/// let m = parse_aliases("gpt-4o=主力\n裸id\nid2=别名=含等号");
/// assert_eq!(m["gpt-4o"], "主力");
/// assert_eq!(m["裸id"], "");
/// assert_eq!(m["id2"], "别名=含等号"); // 首个 '=' 为分隔，ID 不含 '=' 故安全
/// let text = fmt_aliases(&BTreeMap::from([("a".to_string(), "A".to_string()), ("b".to_string(), String::new())]));
/// assert_eq!(text, "a=A\nb");
/// assert_eq!(parse_aliases(&fmt_aliases(&m)), m);
/// ```
pub fn parse_aliases(text: &str) -> BTreeMap<String, String> {
    text.lines().fold(BTreeMap::new(), |mut acc, line| {
        let line = line.trim();
        if line.is_empty() {
            return acc;
        }
        // 首个 '=' 前为模型 ID：ID 本身不含 '='，别名含 '=' 也能整体保留
        match line.split_once('=') {
            Some((id, alias)) => {
                acc.insert(id.trim().to_string(), alias.trim().to_string());
            }
            None => {
                acc.insert(line.to_string(), String::new());
            }
        }
        acc
    })
}

/// [`parse_aliases`] 的反向：别名空只写 ID。
pub fn fmt_aliases(map: &BTreeMap<String, String>) -> String {
    map.iter()
        .map(|(id, alias)| {
            if alias.is_empty() {
                id.clone()
            } else {
                format!("{id}={alias}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Providers {
    /// 读整包；未存过 = 空档案。
    pub fn load(db: &Db) -> Result<Self, Box<dyn Error>> {
        Ok(match db.kv_get(KV_KEY)? {
            Some(json) => serde_json::from_str(&json)?,
            None => Self::default(),
        })
    }

    fn store(&self, db: &Db) -> Result<(), Box<dyn Error>> {
        db.kv_set(KV_KEY, &serde_json::to_string(self)?)
    }

    /// 未删除条目（列表 UI 数据源含灰显项，此迭代器面向"可用"语义）。
    pub fn live(&self) -> impl Iterator<Item = &ProviderEntry> {
        self.list.iter().filter(|p| !p.deleted)
    }

    pub fn find(&self, id: &str) -> Option<&ProviderEntry> {
        self.list.iter().find(|p| p.id == id)
    }

    /// 使用中条目（active 指向已删/不存在 = `None`）。
    pub fn active_entry(&self) -> Option<&ProviderEntry> {
        self.find(&self.active).filter(|p| !p.deleted)
    }

    /// 组装「使用中提供商」的扁平运行视图（下游 `Client`/模型下拉零感知消费）。
    ///
    /// 地址 =（base 或 kind 内置端点）+ 后缀；api 受定制锁覆盖；密钥取 `[keys]` 中本条目的。
    pub fn view(&self, secrets: &Secrets) -> crate::ai::config::Config {
        use crate::ai::config::Config;
        let entry = self.active_entry();
        let (kind, base, suffix, api, stream, models) = match entry {
            Some(e) => (
                e.kind,
                if e.base_url.is_empty() {
                    e.kind.default_base_url().unwrap_or("").to_string()
                } else {
                    e.base_url.clone()
                },
                e.url_suffix.clone(),
                e.api,
                e.stream,
                e.models.clone(),
            ),
            // 无可用条目：给个必然过不了 validate 的空视图（错误文案由调用方出）
            None => (
                Provider::default(),
                String::new(),
                String::new(),
                None,
                true,
                BTreeMap::new(),
            ),
        };
        Config {
            provider: kind,
            base_url: join_url(&base, &suffix),
            api_key: entry
                .and_then(|e| secrets.keys.get(&e.id))
                .cloned()
                .unwrap_or_default(),
            model: self.active_model.clone(),
            api,
            stream,
            models,
        }
    }

    /// 新建条目并持久化，返回其 id（默认不切换使用中；MiMo 预填内置端点便于查看）。
    pub fn create(db: &Db, kind: Provider) -> Result<String, Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        let n: u32 = ps
            .list
            .iter()
            .filter_map(|p| p.id.strip_prefix('p'))
            .filter_map(|s| s.parse().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("p{n}");
        ps.list.push(ProviderEntry {
            id: id.clone(),
            kind,
            name: kind.display_name().to_string(),
            base_url: kind.default_base_url().unwrap_or("").to_string(),
            ..Default::default()
        });
        ps.store(db)?;
        Ok(id)
    }

    /// 「使用此提供商」：active 指针切换（已删条目不可激活）。
    pub fn set_active(db: &Db, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        if ps.find(id).is_some_and(|p| !p.deleted) {
            ps.active = id.to_string();
            ps.store(db)?;
        }
        Ok(())
    }

    /// 更新全局当前模型（模型下拉切换的回写路径）。
    pub fn set_active_model(db: &Db, model: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        ps.active_model = model.to_string();
        ps.store(db)
    }

    /// 详情卡「完成」：按 id 整条替换（不存在则忽略）。
    pub fn save_entry(db: &Db, entry: &ProviderEntry) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        if let Some(slot) = ps.list.iter_mut().find(|p| p.id == entry.id) {
            *slot = entry.clone();
        }
        ps.store(db)
    }

    /// 逻辑删除；删的正是 active 时自动切到下一个未删条目（无则清空指针）。
    pub fn soft_delete(db: &Db, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        if let Some(e) = ps.list.iter_mut().find(|p| p.id == id) {
            e.deleted = true;
        }
        let fallback = ps.live().next().map(|p| p.id.clone());
        if ps.active == id {
            ps.active = fallback.unwrap_or_default();
        }
        ps.store(db)
    }

    /// 恢复逻辑删除条目。
    pub fn restore(db: &Db, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        if let Some(e) = ps.list.iter_mut().find(|p| p.id == id) {
            e.deleted = false;
        }
        ps.store(db)
    }

    /// 彻底删除：物理移除条目并连带清除其在 `[keys]` 中的密钥。
    pub fn purge(db: &Db, secrets_path: impl AsRef<Path>, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        ps.list.retain(|p| p.id != id);
        let fallback = ps.live().next().map(|p| p.id.clone());
        if ps.active == id {
            ps.active = fallback.unwrap_or_default();
        }
        ps.store(db)?;
        let mut secrets: Secrets =
            super::config::load_toml(secrets_path.as_ref()).unwrap_or_default();
        secrets.keys.remove(id);
        super::config::store_toml(&secrets, secrets_path)
    }

    /// 一次性全量迁移（幂等）：旧单配置 → `p1` 条目 + `[keys]`，
    /// 随后**删除**旧 kv `config` 行、`config.toml` 重写为纯 `[keys]`。
    /// 已有 `providers` 键 = 迁移过（或新用户已保存），直接返回。
    ///
    /// 用真实落盘断言「旧的不留」：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::ai::providers::Providers;
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// let path = std::env::temp_dir().join("o_wakaka_migrate_doctest.toml");
    ///
    /// // 旧态：kv("config") 存设置 JSON，toml 存顶级 api_key
    /// db.kv_set("config", r#"{"provider":"custom","base_url":"https://api.x/v1","model":"gpt-4o","stream":true,"models":{"gpt-4o":"主力"}}"#)?;
    /// std::fs::write(&path, "api_key = \"sk-9\"\n")?;
    ///
    /// Providers::migrate_legacy(&db, &path)?;
    /// Providers::migrate_legacy(&db, &path)?;   // 幂等：二次无副作用
    ///
    /// let ps = Providers::load(&db)?;
    /// assert_eq!(ps.active, "p1");
    /// assert_eq!(ps.active_model, "gpt-4o");
    /// let e = ps.active_entry().unwrap();
    /// assert_eq!((e.base_url.as_str(), e.name.as_str()), ("https://api.x/v1", "通用兼容"));
    /// assert_eq!(e.models["gpt-4o"], "主力");
    /// // 旧配置彻底不留：kv("config") 已删，toml 只剩 [keys]
    /// assert_eq!(db.kv_get("config")?, None);
    /// let slim = std::fs::read_to_string(&path)?;
    /// assert!(slim.contains("[keys]") && !slim.contains("api_key ="), "{slim}");
    /// assert!(slim.contains("sk-9"));  // 密钥迁入了 keys
    /// // 视图组装端到端：地址、密钥、别名、当前模型全部来自新结构
    /// let s: o_wakaka::ai::config::Secrets = o_wakaka::ai::config::load_toml(&path)?;
    /// let view = ps.view(&s);
    /// assert_eq!((view.base_url.as_str(), view.api_key.as_str(), view.model.as_str()),
    ///            ("https://api.x/v1", "sk-9", "gpt-4o"));
    /// std::fs::remove_file(&path).ok();
    /// # Ok(()) }
    /// ```
    pub fn migrate_legacy(db: &Db, secrets_path: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
        if db.kv_get(KV_KEY)?.is_some() {
            return Ok(());
        }
        let path = secrets_path.as_ref();
        // 设置：优先旧 kv JSON（DB-2 态），回落老全量 toml（DB-2 之前的用户）
        let mut cfg: Option<crate::ai::config::Config> = match db.kv_get(LEGACY_KV_KEY)? {
            Some(json) => serde_json::from_str(&json).ok(),
            None => super::config::load_toml(path).ok(),
        };
        // 密钥：DB-2 态 toml 是顶级 api_key 字段（Config 视图里可能没读到）
        if let Some(c) = &mut cfg
            && c.api_key.is_empty()
        {
            #[derive(Deserialize)]
            struct LegacyKey {
                #[serde(default)]
                api_key: String,
            }
            if let Ok(k) = super::config::load_toml::<LegacyKey>(path) {
                c.api_key = k.api_key;
            }
        }
        let Some(cfg) = cfg else {
            return Ok(()); // 新用户：无可迁内容，空档案懒生成
        };
        let id = "p1".to_string();
        let entry = ProviderEntry {
            id: id.clone(),
            kind: cfg.provider,
            name: cfg.provider.display_name().to_string(),
            base_url: cfg.base_url,
            url_suffix: String::new(),
            api: cfg.api,
            stream: cfg.stream,
            models: cfg.models,
            deleted: false,
        };
        let mut secrets: Secrets = super::config::load_toml(path).unwrap_or_default();
        if !cfg.api_key.is_empty() {
            secrets.keys.insert(id.clone(), cfg.api_key);
        }
        Providers {
            active: id,
            active_model: cfg.model,
            list: vec![entry],
        }
        .store(db)?;
        super::config::store_toml(&secrets, path)?;
        db.kv_delete(LEGACY_KV_KEY)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_and_active_fallback_chain() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let path = std::env::temp_dir().join("o_wakaka_prov_crud_test.toml");
        std::fs::write(&path, "[keys]\n")?;

        let a = Providers::create(&db, Provider::Custom)?;
        let b = Providers::create(&db, Provider::XiaomiMimo)?;
        assert_eq!((a.as_str(), b.as_str()), ("p1", "p2"));
        let ps = Providers::load(&db)?;
        assert!(ps.active.is_empty(), "新建不自动激活");

        Providers::set_active(&db, &b)?;
        let ps = Providers::load(&db)?;
        assert_eq!(ps.active_entry().unwrap().kind, Provider::XiaomiMimo);
        assert_eq!(
            ps.active_entry().unwrap().base_url,
            "https://api.xiaomimimo.com/v1",
            "MiMo 建条目预填内置端点"
        );

        // 删除 active → 自动回落到剩余可用
        Providers::soft_delete(&db, &b)?;
        let ps = Providers::load(&db)?;
        assert_eq!(ps.active, a);
        // 恢复 + 彻底删除连带清密钥
        Providers::restore(&db, &b)?;
        let mut s = Secrets::default();
        s.keys.insert(b.clone(), "sk-b".into());
        store_toml_save(&s, &path)?;
        Providers::purge(&db, &path, &b)?;
        let ps = Providers::load(&db)?;
        assert!(ps.find(&b).is_none());
        let s: Secrets = super::super::config::load_toml(&path)?;
        assert!(!s.keys.contains_key(&b), "密钥连带清除");
        std::fs::remove_file(&path).ok();
        Ok(())
    }

    fn store_toml_save(s: &Secrets, p: &Path) -> Result<(), Box<dyn Error>> {
        super::super::config::store_toml(s, p)
    }
}
