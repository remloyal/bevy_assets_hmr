//! Multi-format config loader supporting both `ron` and `json` by file extension.

use crate::asset::ConfigAsset;
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::reflect::TypePath;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::sync::Arc;

/// Generic loader for [`ConfigAsset<T>`]. Dispatches on file extension to
/// either `ron::from_bytes` or `serde_json::from_slice`.
///
/// `T` must implement `Asset + DeserializeOwned + Send + Sync + 'static`.
/// The consuming crate registers one `ConfigLoader::<T>` per config type
/// via [`crate::ConfigHmrPlugin::register_hmr_type`].
#[derive(TypePath)]
pub struct ConfigLoader<T> {
    _marker: PhantomData<T>,
}

impl<T> Default for ConfigLoader<T> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

/// Loader error: covers IO, ron parse, and json parse failures.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoaderError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ron parse error: {0}")]
    Ron(#[from] ron::error::SpannedError),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported file extension: {0}")]
    UnsupportedExtension(String),
}

impl<T: Asset + DeserializeOwned + Send + Sync + 'static> AssetLoader for ConfigLoader<T> {
    type Asset = ConfigAsset<T>;
    type Settings = ();
    type Error = Arc<ConfigLoaderError>;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| Arc::new(ConfigLoaderError::Io(e)))?;

        let path = load_context.path().to_string();
        let ext = load_context
            .path()
            .get_extension()
            .unwrap_or_default()
            .to_lowercase();

        let raw: T = match ext.as_str() {
            "ron" => ron::de::from_bytes(&bytes)
                .map_err(|e| Arc::new(ConfigLoaderError::Ron(e)))?,
            "json" => serde_json::from_slice(&bytes)
                .map_err(|e| Arc::new(ConfigLoaderError::Json(e)))?,
            other => {
                return Err(Arc::new(ConfigLoaderError::UnsupportedExtension(
                    other.to_string(),
                )))
            }
        };

        Ok(ConfigAsset {
            raw,
            source_path: path,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["ron", "json"]
    }
}
