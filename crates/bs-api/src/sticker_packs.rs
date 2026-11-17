//! Стикерпаки Rustspace (`/api/rustspace/sticker-packs…`): свои наборы
//! стикеров и добавленные чужие.

use serde::Deserialize;
use serde_json::json;

use crate::client::ApiClient;
use crate::compose::PersonalSticker;
use crate::error::ApiError;

/// Стикерпак.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StickerPack {
    /// Идентификатор.
    pub id: String,
    /// Название.
    pub name: String,
    /// Владелец.
    #[serde(default)]
    pub owner_id: String,
    /// Имя владельца.
    #[serde(default)]
    pub owner_name: Option<String>,
    /// Можно ли добавить другим.
    #[serde(default)]
    pub shared: bool,
    /// Свой пак.
    #[serde(default)]
    pub mine: bool,
    /// Добавлен к себе.
    #[serde(default)]
    pub added: bool,
    /// Стикеры по порядку.
    #[serde(default)]
    pub stickers: Vec<PersonalSticker>,
}

/// Свои и добавленные паки.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StickerPacks {
    /// Свои.
    #[serde(default)]
    pub own: Vec<StickerPack>,
    /// Добавленные чужие.
    #[serde(default)]
    pub added: Vec<StickerPack>,
}

const BASE: &str = "/api/rustspace/sticker-packs";

impl ApiClient {
    /// Свои и добавленные стикерпаки.
    ///
    /// # Errors
    /// Ошибки запроса (сервер без паков отвечает 404).
    pub async fn sticker_packs(&self) -> Result<StickerPacks, ApiError> {
        self.get_json(BASE).await
    }

    /// Создаёт пустой пак.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn create_sticker_pack(&self, name: &str) -> Result<StickerPack, ApiError> {
        self.post_json(BASE, &json!({ "name": name })).await
    }

    /// Переименовывает пак и/или меняет доступность.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn update_sticker_pack(
        &self,
        id: &str,
        name: Option<&str>,
        shared: Option<bool>,
    ) -> Result<StickerPack, ApiError> {
        let mut body = serde_json::Map::new();
        if let Some(name) = name {
            body.insert("name".into(), name.into());
        }
        if let Some(shared) = shared {
            body.insert("shared".into(), shared.into());
        }
        self.patch_json(&format!("{BASE}/{id}"), &body).await
    }

    /// Удаляет свой пак.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn delete_sticker_pack(&self, id: &str) -> Result<serde_json::Value, ApiError> {
        self.delete_json(&format!("{BASE}/{id}")).await
    }

    /// Кладёт стикер (уже на инстансе) в свой пак.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn put_pack_sticker(
        &self,
        pack: &str,
        sticker: &str,
    ) -> Result<StickerPack, ApiError> {
        self.put_json(&format!("{BASE}/{pack}/stickers/{sticker}"), &json!({}))
            .await
    }

    /// Убирает стикер из своего пака.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn remove_pack_sticker(
        &self,
        pack: &str,
        sticker: &str,
    ) -> Result<StickerPack, ApiError> {
        self.delete_json(&format!("{BASE}/{pack}/stickers/{sticker}"))
            .await
    }

    /// Новый порядок стикеров.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn reorder_sticker_pack(
        &self,
        pack: &str,
        order: &[String],
    ) -> Result<StickerPack, ApiError> {
        self.put_json(
            &format!("{BASE}/{pack}/order"),
            &json!({ "stickers": order }),
        )
        .await
    }

    /// Добавляет чужой открытый пак к себе (`added = true`) или убирает.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn set_sticker_pack_added(
        &self,
        pack: &str,
        added: bool,
    ) -> Result<serde_json::Value, ApiError> {
        let path = format!("{BASE}/{pack}/added");
        if added {
            self.put_json(&path, &json!({})).await
        } else {
            self.delete_json(&path).await
        }
    }

    /// Открытые паки, в которых есть стикер.
    ///
    /// # Errors
    /// Ошибки запроса.
    pub async fn sticker_packs_with(&self, sticker: &str) -> Result<Vec<StickerPack>, ApiError> {
        self.get_json(&format!("{BASE}/with/{sticker}")).await
    }
}
