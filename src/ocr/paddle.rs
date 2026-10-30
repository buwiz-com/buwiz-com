//! PaddleOCR PP-OCRv5 mobile (text detection, then Latin-script
//! recognition) on ONNX Runtime, through `oar-ocr`. ONNX Runtime is linked
//! statically; the models are files read from `OCR_MODEL_DIR` on the first
//! photo (`scripts/ocr-models.sh` fetches them, pinned and checked).

use std::path::PathBuf;
use std::sync::Mutex;

use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::prelude::{OAROCR, OAROCRBuilder, TextRegion};

use super::extract::Line;
use super::{Bitmap, Engine};

pub const DETECTION_MODEL: &str = "pp-ocrv5_mobile_det.onnx";
pub const RECOGNITION_MODEL: &str = "latin_pp-ocrv5_mobile_rec.onnx";
pub const DICTIONARY: &str = "ppocrv5_latin_dict.txt";

pub struct PaddleOcr {
    models: PathBuf,
    /// Loaded on first use and kept. Loading them for each document costs
    /// one to five seconds more, and the memory is not reliably given back
    /// between documents anyway.
    pipeline: Mutex<Option<OAROCR>>,
}

impl PaddleOcr {
    pub fn new(models: PathBuf) -> Self {
        Self { models, pipeline: Mutex::new(None) }
    }

    fn load(&self) -> Result<OAROCR, String> {
        let path = |name: &str| self.models.join(name);
        for name in [DETECTION_MODEL, RECOGNITION_MODEL, DICTIONARY] {
            if !path(name).is_file() {
                tracing::error!(models = %self.models.display(), name, "an OCR model file is missing");
                return Err("the OCR models are missing on the server".to_owned());
            }
        }
        // One thread: the web server keeps the other core.
        let session = OrtSessionConfig::new().with_intra_threads(1).with_inter_threads(1);
        OAROCRBuilder::new(path(DETECTION_MODEL), path(RECOGNITION_MODEL), path(DICTIONARY))
            .ort_session(session)
            .build()
            .map_err(|error| {
                tracing::error!(error = %error, "could not load the OCR models");
                "the OCR models could not be loaded".to_owned()
            })
    }
}

impl Engine for PaddleOcr {
    fn name(&self) -> &'static str {
        "paddleocr"
    }

    fn version(&self) -> &'static str {
        "PP-OCRv5 mobile, latin, oar-ocr 0.10"
    }

    fn read(&self, bitmap: &Bitmap) -> Result<Vec<Line>, String> {
        let mut pipeline = self.pipeline.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pipeline.is_none() {
            *pipeline = Some(self.load()?);
        }
        let image = image::RgbImage::from_raw(bitmap.width, bitmap.height, bitmap.rgb.clone())
            .ok_or_else(|| "the photo could not be prepared".to_owned())?;
        let results = pipeline.as_ref().expect("loaded above").predict(vec![image]).map_err(|error| {
            tracing::error!(error = %error, "OCR failed");
            "the OCR engine failed on this photo".to_owned()
        })?;
        Ok(rows(results.iter().flat_map(|result| result.text_regions.iter())))
    }
}

/// Text boxes on the same row joined into one line, left to right, rows top
/// to bottom: a label and its value read as `TIN 123-456-789`.
fn rows<'a>(regions: impl Iterator<Item = &'a TextRegion>) -> Vec<Line> {
    struct Piece {
        top: f32,
        bottom: f32,
        left: f32,
        text: String,
        confidence: f32,
    }
    let mut pieces: Vec<Piece> = regions
        .filter_map(|region| {
            let text = region.text.as_deref()?.trim();
            (!text.is_empty()).then(|| Piece {
                top: region.bounding_box.y_min(),
                bottom: region.bounding_box.y_max(),
                left: region.bounding_box.x_min(),
                text: text.to_owned(),
                confidence: region.confidence.unwrap_or(0.0),
            })
        })
        .collect();
    pieces.sort_by(|a, b| (a.top + a.bottom).total_cmp(&(b.top + b.bottom)));
    let mut rows: Vec<Vec<Piece>> = Vec::new();
    for piece in pieces {
        let middle = (piece.top + piece.bottom) / 2.0;
        match rows.last_mut() {
            // Same row: the middle of this box lies within the row's first box.
            Some(row) if middle > row[0].top && middle < row[0].bottom => row.push(piece),
            _ => rows.push(vec![piece]),
        }
    }
    rows.into_iter()
        .map(|mut row| {
            row.sort_by(|a, b| a.left.total_cmp(&b.left));
            let confidence = row.iter().map(|piece| piece.confidence).fold(1.0, f32::min);
            let text = row.into_iter().map(|piece| piece.text).collect::<Vec<_>>().join(" ");
            Line::new(text, confidence)
        })
        .collect()
}
