use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const MAX_IMAGE_FILE_SIZE: u64 = 50 * 1024 * 1024;

pub struct ViewImageTool;

impl ViewImageTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for ViewImageTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "view_image".to_string(),
            description: "View a local image from the filesystem. Use this when the user asks what a local image file looks like, or when visual inspection of a local image is needed."
                .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "path".to_string(),
                    description: "Local filesystem path to an image file".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "detail".to_string(),
                    description: "Optional detail override: high or original. Omit for high resized behavior."
                        .to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["path"])?;
        match get_string_param(params, "detail").as_deref() {
            None | Some("high") | Some("original") => Ok(()),
            Some(detail) => Err(ToolError::Validation(format!(
                "detail only supports 'high' or 'original', got '{}'",
                detail
            ))),
        }
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path = get_string_param(&params, "path")
            .ok_or_else(|| ToolError::Validation("path is required".to_string()))?;
        let preserve_original = matches!(
            get_string_param(&params, "detail").as_deref(),
            Some("original")
        );
        let path_ref = Path::new(&path);

        if !path_ref.exists() {
            return Err(ToolError::NotFound(format!("Image not found: {}", path)));
        }
        if !path_ref.is_file() {
            return Err(ToolError::Validation(format!(
                "Image path is not a file: {}",
                path
            )));
        }

        let metadata = std::fs::metadata(path_ref)
            .map_err(|err| ToolError::Execution(format!("Failed to read image metadata: {err}")))?;
        if metadata.len() > MAX_IMAGE_FILE_SIZE {
            return Err(ToolError::Execution(format!(
                "Image is too large ({}MB > {}MB limit)",
                metadata.len() / (1024 * 1024),
                MAX_IMAGE_FILE_SIZE / (1024 * 1024)
            )));
        }

        let image =
            crate::utils::image_attachment::prompt_image_for_path(path_ref, preserve_original)
                .map_err(|err| ToolError::Execution(format!("Failed to process image: {err}")))?;

        let detail = if preserve_original {
            "original"
        } else {
            "high"
        };
        let output = format!(
            "Viewed image {} ({}x{}, {})",
            path, image.width, image.height, image.media_type
        );

        Ok(ToolResult::new(format!("Viewed Image: {}", path), output)
            .with_metadata("path", serde_json::json!(path))
            .with_metadata("width", serde_json::json!(image.width))
            .with_metadata("height", serde_json::json!(image.height))
            .with_metadata("media_type", serde_json::json!(image.media_type.clone()))
            .with_metadata("detail", serde_json::json!(detail))
            .with_image(image.data_url, image.media_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn test_context() -> ToolContext {
        let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        ToolContext::new("session", "message", "build", abort_rx)
    }

    #[tokio::test]
    async fn view_image_returns_model_image_content() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("example.png");
        let image = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 255]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode png");
        std::fs::write(&path, encoded.into_inner()).expect("write png");

        let result = ViewImageTool::new()
            .execute(
                serde_json::json!({
                    "path": path.to_string_lossy(),
                }),
                &test_context(),
            )
            .await
            .expect("view image");

        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, "image/png");
        assert!(result.images[0]
            .data_url
            .starts_with("data:image/png;base64,"));
        assert_eq!(result.metadata["width"], serde_json::json!(2));
        assert_eq!(result.metadata["height"], serde_json::json!(1));
    }
}
