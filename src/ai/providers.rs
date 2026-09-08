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
//! 条目删除 = 两步确认后**物理移除**并连带清 `[keys]` 中该 id 的密钥
//! （历史遗留的 `deleted` 灰显条目仍可恢复/彻底删除）。
//! # 启停开关
//! 条目另有 `enabled`（默认启用）：禁用 = 暂离线——不进聊天模型下拉、不可被路由；
//! 禁用当前路由项时 active 自动回落到下一个可用条目。

use std::{collections::BTreeMap, error::Error, path::Path};

use serde::{Deserialize, Serialize};

use super::config::{Api, Secrets};
use super::provider::Provider;
use crate::db::Db;

/// kv 表中的整包键名。
const KV_KEY: &str = "providers";
/// 旧配置（DB-2 单配置时代）在 kv 表中的键名，仅迁移时读取/删除。
const LEGACY_KV_KEY: &str = "config";

/// 一个可用模型（Step B：从「id→别名」升级为带能力标注的对象）。
/// 能力位此期**只存只显**（勾选编辑闭环），消费（按能力裁剪请求/禁用开关）在 Step C。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    /// 请求名（发给网关的 model 字段）
    pub id: String,
    /// 显示别名（空 = 原样显示 id）
    pub alias: String,
    /// 能力：思考/推理
    pub thinking: bool,
    /// 能力：视觉（图片输入）
    pub vision: bool,
    /// 能力：语音
    pub audio: bool,
    /// 能力：视频
    pub video: bool,
    /// 能力：工具调用
    pub tools: bool,
}

/// Step A 的 models 旧格式（`{"id":"别名"}` 对象）兼容：读出即升级为对象列表。
fn de_models<'de, D>(d: D) -> Result<Vec<ModelInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Map(BTreeMap<String, String>),
        List(Vec<ModelInfo>),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Map(m) => m
            .into_iter()
            .map(|(id, alias)| ModelInfo {
                id,
                alias,
                ..Default::default()
            })
            .collect(),
        Raw::List(v) => v,
    })
}

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
    /// 可用模型列表（按 id 升序持久化；兼容 Step A 的 map 旧格式）
    #[serde(default, deserialize_with = "de_models")]
    pub models: Vec<ModelInfo>,
    /// 逻辑删除标记（删除宪法；仅供历史遗留条目，UI 新删除直接物理 purge）
    pub deleted: bool,
    /// 启停开关（false = 暂离线：不进聊天下拉、不可路由；旧档案缺字段视为启用）
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
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
            models: Vec::new(),
            deleted: false,
            enabled: true,
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

    /// 未删除条目（列表 UI 数据源含灰显/禁用项，此迭代器面向"存在"语义）。
    pub fn live(&self) -> impl Iterator<Item = &ProviderEntry> {
        self.list.iter().filter(|p| !p.deleted)
    }

    /// 未删且启用的条目（对话侧可用面：模型下拉聚合、active 回落候选）。
    pub fn usable(&self) -> impl Iterator<Item = &ProviderEntry> {
        self.list.iter().filter(|p| !p.deleted && p.enabled)
    }

    pub fn find(&self, id: &str) -> Option<&ProviderEntry> {
        self.list.iter().find(|p| p.id == id)
    }

    /// 未删条目按 id 取可变引用（写路径守卫：已删/不存在回 `None`）。
    fn live_mut(&mut self, id: &str) -> Option<&mut ProviderEntry> {
        self.list.iter_mut().find(|p| p.id == id && !p.deleted)
    }

    /// 置/清某条目的逻辑删除标记（id 不存在则忽略）。
    fn mark_deleted(&mut self, id: &str, deleted: bool) {
        if let Some(e) = self.list.iter_mut().find(|p| p.id == id) {
            e.deleted = deleted;
        }
    }

    /// 删除/彻底删除后维护 active：若 active 正指向被处理项，切到下一个可用（未删且启用）条目（无则清空）。
    fn reactivate_if_current(&mut self, id: &str) {
        if self.active == id {
            let next = self.usable().next().map(|p| p.id.clone());
            self.active = next.unwrap_or_default();
        }
    }

    /// 启用/禁用条目（已删条目忽略）。禁用正被路由的 active → 自动回落到下一个可用条目。
    pub fn set_enabled(db: &Db, id: &str, on: bool) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        let Some(e) = ps.list.iter_mut().find(|p| p.id == id && !p.deleted) else {
            return Ok(());
        };
        e.enabled = on;
        if !on && ps.active == id {
            let next = ps.usable().next().map(|p| p.id.clone());
            ps.active = next.unwrap_or_default();
        }
        ps.store(db)
    }

    /// 使用中条目（active 指向已删/已禁用/不存在 = `None`）。
    pub fn active_entry(&self) -> Option<&ProviderEntry> {
        self.find(&self.active).filter(|p| !p.deleted && p.enabled)
    }

    /// 组装「使用中提供商」的扁平运行视图（下游 `Client`/模型下拉零感知消费）。
    ///
    /// 地址 =（base 或 kind 内置端点）+ 后缀；api 受定制锁覆盖；密钥取 `[keys]` 中本条目的。
    pub fn view(&self, secrets: &Secrets) -> crate::ai::config::Config {
        use crate::ai::config::Config;
        let entry = self.active_entry();
        // 视图仍暴露 id→别名 映射给下游（空别名也收录，下拉才能列出未命名模型；能力位 Step C 才进请求）
        let models: BTreeMap<String, String> = entry
            .map(|e| {
                e.models
                    .iter()
                    .map(|m| (m.id.clone(), m.alias.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let (kind, base, suffix, api, stream) = match entry {
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
            ),
            // 无可用条目：给个必然过不了 validate 的空视图（错误文案由调用方出）
            None => (
                Provider::default(),
                String::new(),
                String::new(),
                None,
                true,
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

    /// 新建条目并持久化，返回其 id。首家（active 空）自动激活——「添加了就是能用的」；
    /// 后续家不抢当前路由（MiMo 预填内置端点便于查看）。
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
        if ps.active.is_empty() {
            ps.active = id.clone();
        }
        ps.store(db)?;
        Ok(id)
    }

    /// 路由指针切换（下拉选中某提供商模型时隐式调用；已删/已禁用不可激活）。
    pub fn set_active(db: &Db, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        if ps.find(id).is_some_and(|p| !p.deleted && p.enabled) {
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

    /// 增改模型（按 id 匹配，整条覆写；列表按 id 升序保持持久化稳定）。
    ///
    /// 行级即时落盘 + 旧 map 格式兼容升级，一并验证：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::ai::provider::Provider;
    /// use o_wakaka::ai::providers::{ModelInfo, Providers};
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// let pid = Providers::create(&db, Provider::Custom)?;
    /// Providers::upsert_model(&db, &pid, ModelInfo { id: "b-model".into(), alias: "B牌".into(), tools: true, ..Default::default() })?;
    /// Providers::upsert_model(&db, &pid, ModelInfo { id: "a-model".into(), ..Default::default() })?;
    /// let e = Providers::load(&db)?.find(&pid).unwrap().clone();
    /// assert_eq!(e.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a-model", "b-model"]); // 按 id 排序
    /// // 同 id 再存 = 覆写不重复
    /// Providers::upsert_model(&db, &pid, ModelInfo { id: "b-model".into(), alias: "B牌改".into(), ..Default::default() })?;
    /// let e = Providers::load(&db)?.find(&pid).unwrap().clone();
    /// assert_eq!((e.models.len(), e.models[1].alias.as_str()), (2, "B牌改"));
    /// assert!(!e.models[1].tools, "覆写整条：未勾能力即清除");
    ///
    /// // 旧 Step A map 格式条目读出即升级
    /// db.kv_set("providers", r#"{"active":"p9","active_model":"","list":[
    ///   {"id":"p9","kind":"custom","name":"旧","base_url":"u","url_suffix":"","api":null,
    ///    "stream":true,"models":{"x":"小x"},"deleted":false}]}"#)?;
    /// let e = Providers::load(&db)?.find("p9").unwrap().clone();
    /// assert_eq!((e.models.len(), e.models[0].id.as_str(), e.models[0].alias.as_str()), (1, "x", "小x"));
    /// # Ok(()) }
    /// ```
    pub fn upsert_model(db: &Db, prov_id: &str, m: ModelInfo) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        let Some(e) = ps.live_mut(prov_id) else {
            return Err("提供商不存在或已删除".into());
        };
        match e.models.iter_mut().find(|x| x.id == m.id) {
            Some(slot) => *slot = m,
            None => e.models.push(m),
        }
        e.models.sort_by(|a, b| a.id.cmp(&b.id));
        ps.store(db)
    }

    /// 删除模型（逻辑删仅到提供商一级，模型直接物理移除——列表项可重加）；
    /// 若正删的是全局当前模型则一并清空指针。
    pub fn remove_model(db: &Db, prov_id: &str, model_id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        let Some(e) = ps.live_mut(prov_id) else {
            return Ok(());
        };
        e.models.retain(|m| m.id != model_id);
        if ps.active_model == model_id && ps.active == *prov_id {
            ps.active_model = String::new();
        }
        ps.store(db)
    }

    /// 逻辑删除；删的正是 active 时自动切到下一个未删条目（无则清空指针）。
    pub fn soft_delete(db: &Db, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        ps.mark_deleted(id, true);
        ps.reactivate_if_current(id);
        ps.store(db)
    }

    /// 恢复逻辑删除条目。
    pub fn restore(db: &Db, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        ps.mark_deleted(id, false);
        ps.store(db)
    }

    /// 彻底删除：物理移除条目并连带清除其在 `[keys]` 中的密钥。
    pub fn purge(db: &Db, secrets_path: impl AsRef<Path>, id: &str) -> Result<(), Box<dyn Error>> {
        let mut ps = Self::load(db)?;
        ps.list.retain(|p| p.id != id);
        ps.reactivate_if_current(id);
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
    /// assert_eq!(e.models[0].id, "gpt-4o");
    /// assert_eq!(e.models[0].alias, "主力"); // 旧 map 值折成别名字段
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
            models: cfg
                .models
                .into_iter()
                .map(|(id, alias)| ModelInfo {
                    id,
                    alias,
                    ..Default::default()
                })
                .collect(),
            deleted: false,
            enabled: true,
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
        assert_eq!(ps.active, a, "首家自动激活（添加了就是能用的）");

        Providers::set_active(&db, &b)?;
        let ps = Providers::load(&db)?;
        assert_eq!(ps.active_entry().unwrap().kind, Provider::XiaomiMimo);
        assert_eq!(
            ps.active_entry().unwrap().base_url,
            "https://api.xiaomimimo.com/v1",
            "MiMo 建条目预填内置端点"
        );

        // 禁用正被路由的 active → 回落到下一个可用；禁用条目不可再激活
        Providers::set_enabled(&db, &b, false)?;
        let ps = Providers::load(&db)?;
        assert_eq!(ps.active, a, "禁用当前路由项自动回落");
        assert!(!ps.find(&b).unwrap().enabled);
        Providers::set_active(&db, &b)?;
        assert_eq!(Providers::load(&db)?.active, a, "禁用条目不可激活");
        Providers::set_enabled(&db, &b, true)?;
        Providers::set_active(&db, &b)?;
        assert_eq!(Providers::load(&db)?.active, b, "重新启用后可激活");

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
