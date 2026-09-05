//! 运行配置与通用 TOML 配置读写工具。
//!
//! # 存储拆分（DB-2）
//! 正式配置的**设置 JSON**（provider/base_url/model/api/stream/[models] 别名）
//! 存 SQLite [`kv`](crate::db::Db::kv_set) 表（键 `config`），`config.toml` 只
//! 剩 `api_key`——**库里有配置、文件里留凭据**：备份/导出 DB 不泄密。
//! 老式全量 `config.toml` 仍兼容：DB 无配置时作为回落读取源，首次
//! [`Config::save`] 自动完成拆分迁移。
//!
//! 凭据文件约定（均含密钥、已被 git 忽略，模板见项目根 `config.example.toml`）：
//! - `config.toml`：正式代码使用，经 [`Config::load`]（kv + 密钥两路组装）
//! - `config.test.toml`：需真实凭据的测试使用（**保持全量 TOML**，[`load_toml`]
//!   直读不碰 DB；`#[ignore]`，`cargo test -- --ignored`）
//!
//! 文档测试不依赖上述本地文件：用 [`load_toml`]/[`store_toml`] 与
//! [`Db::open_in_memory`] 在临时目录自造配置。

use std::{collections::BTreeMap, error::Error, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::db::Db;

/// 从 TOML 文件读取任意配置类型（通用工具，不限于 [`Config`]）。
pub fn load_toml<T: serde::de::DeserializeOwned>(
    path: impl AsRef<Path>,
) -> Result<T, Box<dyn Error>> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

/// 将任意可序列化配置以 pretty TOML 写入文件（通用工具）。
///
/// 读写往返与 Debug 脱敏一起验证，无需本地凭据文件；注意 [`Config`] 的
/// `api_key` 序列化即丢（skip），密钥落盘只经 [`Secrets`]：
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::collections::BTreeMap;
/// use o_wakaka::ai::config::{Api, Config, load_toml, store_toml};
///
/// let path = std::env::temp_dir().join("o_wakaka_cfg_doctest.toml");
/// let cfg = Config {
///     base_url: "https://api.openai.com/v1".into(),
///     api_key: "sk-test".into(),
///     model: "gpt-4o-mini".into(),
///     api: Some(Api::Responses),
///     models: BTreeMap::from([("gpt-4o-mini".to_string(), "主力模型".to_string())]),
///     ..Config::default()
/// };
/// store_toml(&cfg, &path)?;
/// let raw = std::fs::read_to_string(&path)?;
/// let back: Config = load_toml(&path)?;
/// std::fs::remove_file(&path).ok();
///
/// assert_eq!(back.base_url, cfg.base_url);
/// assert_eq!(back.model, cfg.model);
/// assert_eq!(back.api, Some(Api::Responses));
/// assert_eq!(back.models["gpt-4o-mini"], "主力模型");
/// // 密钥不随 Config 序列化（文件与 DB 同理）；Debug 输出恒脱敏
/// assert!(!raw.contains("sk-test"));
/// assert!(back.api_key.is_empty());
/// assert!(!format!("{cfg:?}").contains(&cfg.api_key));
/// # Ok(())
/// # }
/// ```
pub fn store_toml<T: Serialize>(value: &T, path: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
    fs::write(path, toml::to_string_pretty(value)?)?;
    Ok(())
}

/// 对话接口族：决定请求发往哪个端点族与按哪种线格式收发。
///
/// 配置取值小写字符串；缺省（字段缺失）为 [`Chat`](Self::Chat) 兼容接口，
/// 手动写 `api = "responses"` 才启用 OpenAI Responses API。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    /// OpenAI Responses API（`POST /responses`）
    #[serde(alias = "response")]
    Responses,
    /// OpenAI 兼容 Chat Completions（`POST /chat/completions`，默认保底）
    #[default]
    Chat,
}

/// 提供商：通用兼容端点或厂家定制。定义与各家事实已独立至 [`crate::ai::provider`]。
pub use crate::ai::provider::Provider;

/// `stream` 字段缺省值：流式（serde 要求独立函数）。
fn default_stream() -> bool {
    true
}

/// 运行配置（`config.toml` 顶级表）。
///
/// `provider` 选通用兼容（custom，默认）或厂家定制；`base_url`/`api` 可缺省，
/// 缺省按提供商内置默认解析（见 [`effective_base_url`](Self::effective_base_url)），
/// 老配置零改动仍然可用。`models` 别名表与 `model` 激活项由 UI 读写回存：
///
/// ```
/// use o_wakaka::ai::config::{Api, Config, Provider};
///
/// // custom（默认）：base_url 必填，api 缺省 = chat 保底，stream 缺省 = true
/// let cfg: Config =
///     toml::from_str("base_url=\"u\"\napi_key=\"k\"\nmodel=\"m\"\n").unwrap();
/// assert_eq!(cfg.provider, Provider::Custom);
/// assert_eq!(cfg.effective_api(), Api::Chat);
/// assert!(cfg.stream);
/// assert!(cfg.validate().is_ok());
///
/// // xiaomi_mimo：端点缺省 = 官方内置、接口族锁 responses；显式写 base_url 可覆盖为专属网关
/// let mimo: Config = toml::from_str(
///     "provider=\"xiaomi_mimo\"\napi_key=\"k\"\nmodel=\"mimo-v2.5\"\n",
/// )
/// .unwrap();
/// assert_eq!(mimo.effective_base_url(), "https://api.xiaomimimo.com/v1");
/// assert_eq!(mimo.effective_api(), Api::Responses);
/// let bad: Config = toml::from_str(
///     "provider=\"xiaomi_mimo\"\napi_key=\"k\"\nmodel=\"m\"\napi=\"chat\"\n",
/// )
/// .unwrap();
/// assert!(bad.validate().is_err()); // 定制提供商接口族锁死
///
/// // [models] 别名表：键 = 模型 ID，值 = UI 显示名
/// let aliased: Config = toml::from_str(
///     "base_url=\"u\"\napi_key=\"k\"\nmodel=\"m\"\n[models]\nm = \"主力模型\"\n",
/// )
/// .unwrap();
/// assert_eq!(aliased.models["m"], "主力模型");
/// ```
#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    /// 提供商：`"custom"`（默认）| `"xiaomi_mimo"`（小米 MiMo，端点内置、接口族锁死）
    #[serde(default)]
    pub provider: Provider,
    /// API 根地址，如 `https://api.openai.com/v1`（末尾斜杠可选，客户端会归一化）。
    /// custom 必填；定制提供商缺省用内置端点，显式写了以显式为准
    #[serde(default)]
    pub base_url: String,
    /// Bearer 鉴权密钥：只存本地 `config.toml`（[`Secrets`]），配置 JSON 与
    /// DB 一律不落（序列化恒 skip；反序列化容缺省，密钥由 load 两路组装回填）
    #[serde(default, skip_serializing)]
    pub api_key: String,
    /// 当前激活模型名，随请求体 `model` 字段发送；UI 切换后回写此字段
    pub model: String,
    /// 对话接口族（仅 custom 可自由配）：缺省 `chat`，手动写 `"responses"` 启用新接口
    #[serde(default)]
    pub api: Option<Api>,
    /// 是否流式输出，缺省 `true`；`false` 时整段一次性返回
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// 可用模型别名表（可选）：模型 ID → UI 显示名；未列的 ID 原样显示
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, String>,
}

/// 本地 `config.toml` 的唯一内容：DB-2 起设置进库、凭据留在文件。
///
/// 反序列化容忍多余字段 = 老全量 config.toml 与迁移前文件都能直读密钥。
///
/// ```
/// use o_wakaka::ai::config::Secrets;
///
/// // 从老全量 toml 提取密钥（其余字段忽略）
/// let s: Secrets = toml::from_str("base_url=\"u\"\napi_key=\"sk-x\"\nmodel=\"m\"\n").unwrap();
/// assert_eq!(s.api_key, "sk-x");
/// // 缺密钥字段不炸（空串由 UI 保存路径提示补填）
/// let e: Secrets = toml::from_str("").unwrap();
/// assert!(e.api_key.is_empty());
/// // Debug 恒脱敏（同 Config 纪律）
/// assert!(!format!("{s:?}").contains("sk-x"));
/// ```
#[derive(Serialize, Deserialize, Default)]
pub struct Secrets {
    /// Bearer 鉴权密钥
    #[serde(default)]
    pub api_key: String,
}

/// 手动实现：恒脱敏，防 `{:?}`/日志/panic 泄漏密钥（与 [`Config`] 同纪律）。
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

/// 设置 JSON 在 [`kv`](crate::db::Db::kv_get) 表中的键名。
const KV_KEY: &str = "config";

/// 手动实现：`api_key` 恒为脱敏占位，防止 `{:?}`/日志/panic 输出泄漏密钥。
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("api", &self.api)
            .field("stream", &self.stream)
            .field("models", &self.models)
            .finish()
    }
}

impl Config {
    /// 读运行配置（两路组装）：设置 JSON 取 DB kv `config` 键，密钥取
    /// `config.toml`（相对工作目录）；DB 无配置时回落读老式全量 toml。
    ///
    /// 需真实凭据的测试请改用 [`load_toml`] 直读全量 `config.test.toml`（不碰 DB）。
    pub fn load(db: &Db) -> Result<Self, Box<dyn Error>> {
        Self::load_at(db, "config.toml")
    }

    /// [`load`](Self::load) 的路径参数化版（doctest 自给自足用）。
    ///
    /// 回落与迁移、密钥不进 DB、拆分保存后往返，全部离线可验：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::ai::config::Config;
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// let path = std::env::temp_dir().join("o_wakaka_cfgmerge_doctest.toml");
    ///
    /// // 老全量 toml：DB 空时回落直读，密钥即来自文件
    /// std::fs::write(&path, "base_url=\"u\"\napi_key=\"sk-old\"\nmodel=\"m\"\n")?;
    /// let cfg = Config::load_at(&db, &path)?;
    /// assert_eq!((cfg.base_url.as_str(), cfg.api_key.as_str(), cfg.model.as_str()), ("u", "sk-old", "m"));
    ///
    /// // save：设置进 DB（不含密钥），toml 瘦身为仅 api_key —— 首次保存即完成迁移
    /// cfg.save_at(&db, &path)?;
    /// assert!(!db.kv_get("config")?.unwrap().contains("sk-old"));
    /// assert_eq!(std::fs::read_to_string(&path)?.trim(), "api_key = \"sk-old\"");
    ///
    /// // "重启"后一切如旧：字段读自 DB，密钥读自瘦身文件
    /// let back = Config::load_at(&db, &path)?;
    /// assert_eq!((back.base_url.as_str(), back.api_key.as_str(), back.model.as_str()), ("u", "sk-old", "m"));
    /// std::fs::remove_file(&path).ok();
    /// # Ok(()) }
    /// ```
    pub fn load_at(db: &Db, secrets_path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        let mut cfg: Config = match db.kv_get(KV_KEY)? {
            Some(json) => serde_json::from_str(&json)?,
            None => load_toml(secrets_path.as_ref())?,
        };
        cfg.api_key = load_toml::<Secrets>(secrets_path)?.api_key;
        Ok(cfg)
    }

    /// 保存配置：设置 JSON 写 DB kv（恒不含密钥），`config.toml` 重写为
    /// 仅 [`Secrets`]——对老全量文件即完成一次性拆分迁移（幂等）。
    pub fn save(&self, db: &Db) -> Result<(), Box<dyn Error>> {
        self.save_at(db, "config.toml")
    }

    /// [`save`](Self::save) 的路径参数化版（doctest 自给自足用）。
    pub fn save_at(&self, db: &Db, secrets_path: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
        db.kv_set(KV_KEY, &serde_json::to_string(self)?)?;
        store_toml(
            &Secrets {
                api_key: self.api_key.clone(),
            },
            secrets_path,
        )
    }

    /// 生效根地址：显式写的 `base_url` 优先，其次提供商内置默认；空 = 未配置。
    pub fn effective_base_url(&self) -> &str {
        if self.base_url.is_empty() {
            self.provider.default_base_url().unwrap_or("")
        } else {
            &self.base_url
        }
    }

    /// 生效接口族：定制提供商锁死（[`Provider::locked_api`]）；custom 缺省 Chat。
    pub fn effective_api(&self) -> Api {
        if let Some(locked) = self.provider.locked_api() {
            return locked;
        }
        self.api.unwrap_or_default()
    }

    /// 配置完备性检查，`Err` 携带可直接展示的文案（[`Client::load`](super::client::Client::load) 建客户端前调用）。
    pub fn validate(&self) -> Result<(), String> {
        if self.effective_base_url().is_empty() {
            return Err("缺少 base_url：provider = \"custom\" 时必须显式配置".into());
        }
        if let Some(locked) = self.provider.locked_api()
            && let Some(api) = self.api
            && api != locked
        {
            return Err(format!(
                "定制提供商接口族锁死 {}, api 不能配 \"{}\"",
                api_name(locked),
                api_name(api)
            ));
        }
        Ok(())
    }
}

/// TOML 接口族名（与 [`Api`] 的 serde 改名保持一致），仅用于校验报错文案。
fn api_name(api: Api) -> &'static str {
    match api {
        Api::Chat => "chat",
        Api::Responses => "responses",
    }
}
