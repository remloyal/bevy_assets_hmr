//! Multi-format config loader supporting both `ron` and `json` by file extension.

use crate::asset::ConfigAsset;
use bevy::asset::{Asset, AssetLoader, LoadContext, io::Reader};
use bevy::prelude::Resource;
use bevy::reflect::TypePath;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

type Validator<T> = Arc<dyn Fn(&T) -> Result<(), String> + Send + Sync + 'static>;

/// Runtime validator shared by a config loader and its app resource.
#[derive(Resource, Clone)]
pub struct ConfigValidator<T> {
    callback: Arc<RwLock<Option<Validator<T>>>>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> ConfigValidator<T> {
    pub(crate) fn with_shared(callback: Arc<RwLock<Option<Validator<T>>>>) -> Self {
        Self {
            callback,
            _marker: PhantomData,
        }
    }

    /// Install or replace the validation callback.
    pub fn set<F>(&mut self, validator: F)
    where
        F: Fn(&T) -> Result<(), String> + Send + Sync + 'static,
    {
        if let Ok(mut callback) = self.callback.write() {
            *callback = Some(Arc::new(validator));
        }
    }

    /// Remove the current validation callback.
    pub fn clear(&mut self) {
        if let Ok(mut callback) = self.callback.write() {
            *callback = None;
        }
    }
}

/// Generic loader for [`ConfigAsset<T>`]. Dispatches on file extension to
/// either `ron::from_bytes` or `serde_json::from_slice`.
///
/// `T` must implement `Asset + DeserializeOwned + Send + Sync + 'static`.
/// The consuming crate registers one `ConfigLoader::<T>` per config type
/// via [`crate::ConfigHmrAppExt::register_config`].
#[derive(TypePath)]
pub struct ConfigLoader<T> {
    validator: Arc<RwLock<Option<Validator<T>>>>,
    _marker: PhantomData<T>,
}

impl<T> Default for ConfigLoader<T> {
    fn default() -> Self {
        let validator = Arc::new(RwLock::new(None));
        Self {
            validator,
            _marker: PhantomData,
        }
    }
}

impl<T> ConfigLoader<T> {
    /// Construct a loader and the matching runtime validator resource.
    pub(crate) fn with_validator() -> (Self, ConfigValidator<T>) {
        let shared = Arc::new(RwLock::new(None));
        (
            Self {
                validator: shared.clone(),
                _marker: PhantomData,
            },
            ConfigValidator::with_shared(shared),
        )
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
    #[error("config validation error: {0}")]
    Validation(String),
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
            "ron" => {
                ron::de::from_bytes(&bytes).map_err(|e| Arc::new(ConfigLoaderError::Ron(e)))?
            }
            "json" => {
                serde_json::from_slice(&bytes).map_err(|e| Arc::new(ConfigLoaderError::Json(e)))?
            }
            other => {
                return Err(Arc::new(ConfigLoaderError::UnsupportedExtension(
                    other.to_string(),
                )));
            }
        };

        let validator = self
            .validator
            .read()
            .ok()
            .and_then(|callback| callback.clone());
        if let Some(validator) = validator {
            validator(&raw).map_err(|error| Arc::new(ConfigLoaderError::Validation(error)))?;
        }

        Ok(ConfigAsset {
            raw,
            source_path: path,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["ron", "json"]
    }
}
