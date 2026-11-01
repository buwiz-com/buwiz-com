//! OCR on this server, with the `ocr` feature and the models in
//! `OCR_MODEL_DIR` (default `ocr-models/`, from `scripts/ocr-models.sh`).
//! The ID is synthetic: a TIN-ID-like card drawn here in a plain bitmap
//! font, never a real person's.
//!
//!   scripts/ocr-models.sh && cargo test --features ocr --test ocr

use std::path::PathBuf;
use std::time::{Duration, Instant};

use buwiz_cloud::claims::{self, ClaimKind, DocumentKind, NewClaim, Upload};
use buwiz_cloud::config::{Config, MailConfig};
use buwiz_cloud::link::SenderLink;
use buwiz_cloud::ocr::extract::{self, Kind, Verdict};
use buwiz_cloud::ocr::paddle::PaddleOcr;
use buwiz_cloud::ocr::{self, Engine};
use buwiz_cloud::state::AppState;
use buwiz_cloud::storage::DocumentStore;
use buwiz_cloud::tin::Tin;
use image::{ImageEncoder, Rgb, RgbImage};
use sqlx::PgPool;
use zeroize::Zeroizing;

fn models() -> PathBuf {
    let dir = std::env::var_os("OCR_MODEL_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ocr-models"), PathBuf::from);
    assert!(
        dir.join("pp-ocrv5_mobile_det.onnx").is_file(),
        "no OCR models in {}: run scripts/ocr-models.sh",
        dir.display()
    );
    dir
}

/// Draws `text` with the 8×8 public-domain bitmap font, each dot `scale`
/// pixels square.
fn draw(card: &mut RgbImage, x: u32, y: u32, scale: u32, text: &str, ink: Rgb<u8>) {
    use font8x8::UnicodeFonts;
    for (index, ch) in text.chars().enumerate() {
        let Some(glyph) = font8x8::BASIC_FONTS.get(ch) else { continue };
        let left = x + index as u32 * 8 * scale;
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8 {
                if bits & (1 << column) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (left + column * scale + dx, y + row as u32 * scale + dy);
                        if px < card.width() && py < card.height() {
                            card.put_pixel(px, py, ink);
                        }
                    }
                }
            }
        }
    }
}

/// A card like a TIN ID: a pale background with a border, a coloured band,
/// the headings, TIN, name and birth date, and a grey photo box.
fn tin_card() -> Vec<u8> {
    let (width, height) = (1100, 690);
    let mut card = RgbImage::from_pixel(width, height, Rgb([238, 234, 218]));
    for x in 0..width {
        for y in 0..height {
            if x < 6 || y < 6 || x >= width - 6 || y >= height - 6 {
                card.put_pixel(x, y, Rgb([120, 130, 150]));
            } else if (20..110).contains(&y) {
                card.put_pixel(x, y, Rgb([210, 222, 240]));
            } else if (780..1060).contains(&x) && (170..520).contains(&y) {
                card.put_pixel(x, y, Rgb([170, 170, 175]));
            }
        }
    }
    let ink = Rgb([20, 24, 32]);
    draw(&mut card, 40, 34, 3, "REPUBLIC OF THE PHILIPPINES", ink);
    draw(&mut card, 40, 70, 3, "BUREAU OF INTERNAL REVENUE", ink);
    draw(&mut card, 40, 140, 3, "TAXPAYER IDENTIFICATION", ink);
    draw(&mut card, 40, 220, 4, "TIN 123-456-789-000", ink);
    draw(&mut card, 40, 320, 4, "DELA CRUZ, JUAN SANTOS", ink);
    draw(&mut card, 40, 400, 3, "12 RIZAL ST QUEZON CITY", ink);
    draw(&mut card, 40, 470, 3, "BIRTH DATE 1990/01/15", ink);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(card.as_raw(), width, height, image::ExtendedColorType::Rgb8)
        .unwrap();
    png
}

#[test]
fn reads_the_tin_from_a_synthetic_tin_id() {
    let engine = PaddleOcr::new(models());
    let started = Instant::now();
    let text = ocr::read_file(&engine, "image/png", &tin_card()).unwrap();
    eprintln!("read in {:?} with {} {}: {:?}", started.elapsed(), engine.name(), engine.version(), text.lines);
    let (fields, confidence) = extract::extract(&text.lines);
    assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"), "{:?}", text.lines);
    assert_eq!(fields.kind, Kind::TinId);
    assert!(confidence.tin.is_some_and(|sure| sure > 0.5), "{confidence:?}");
    let typed = Tin::parse("123456789", "").unwrap();
    assert_eq!(extract::compare_tin(&typed, fields.tin.as_deref().unwrap()), Verdict::Match);
    assert_eq!(extract::compare_names("Juan Santos Dela Cruz", fields.name.as_deref().unwrap()), Verdict::Match);
    assert_eq!(fields.birth_date.as_deref(), Some("1990-01-15"));
    // The engine is loaded once; the second photo is quicker.
    let started = Instant::now();
    ocr::read_file(&engine, "image/png", &tin_card()).unwrap();
    eprintln!("read again in {:?}", started.elapsed());
}

/// The card photographed by a phone: 4032×3024 JPEG, the card filling the
/// middle of a darker table. The photo is scaled down before reading.
#[test]
fn reads_a_phone_sized_photo() {
    let card = image::load_from_memory(&tin_card()).unwrap().to_rgb8();
    let card = image::imageops::resize(&card, 2800, 1757, image::imageops::FilterType::Triangle);
    let mut photo = RgbImage::from_pixel(4032, 3024, Rgb([92, 74, 60]));
    image::imageops::overlay(&mut photo, &card, 616, 633);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .write_image(photo.as_raw(), photo.width(), photo.height(), image::ExtendedColorType::Rgb8)
        .unwrap();
    let engine = PaddleOcr::new(models());
    let started = Instant::now();
    let text = ocr::read_file(&engine, "image/jpeg", &jpeg).unwrap();
    eprintln!("read a {} KB phone photo in {:?}", jpeg.len() / 1024, started.elapsed());
    let (fields, _) = extract::extract(&text.lines);
    assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"), "{:?}", text.lines);
}

/// A claim sent with the card is read in the background, and the reading
/// compares with what was typed.
#[sqlx::test(migrations = "./migrations")]
async fn a_claim_is_read_after_it_is_sent(db: PgPool) {
    // The reader finds the models the same way: OCR_MODEL_DIR, or
    // `ocr-models/` here.
    models();
    let config = Config {
        host: "127.0.0.1".into(),
        port: 0,
        database_url: String::new(),
        account_origin: "http://localhost:3008".into(),
        extra_origins: Vec::new(),
        secure_cookies: false,
        document_key: [7; 32],
        admin_emails: Vec::new(),
        mail: MailConfig::Log,
        trust_list: None,
        downloads_url: "http://127.0.0.1:9".into(),
        forms_origin: "https://buwiz.com".into(),
        // Nothing listens there: the star count is unknown.
        github_api_url: "http://127.0.0.1:9".into(),
    };
    let sender = SenderLink::new("http://127.0.0.1:9", Zeroizing::new("unused".into()));
    let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let state = AppState::new(config, db.clone(), DocumentStore::Database, key, sender);
    let account: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('juan@example.test') RETURNING id")
            .fetch_one(&db)
            .await
            .unwrap();
    let claim = NewClaim {
        kind: ClaimKind::Personal,
        tin: Tin::parse("123-456-789", "").unwrap(),
        registered_name: "Juan Santos Dela Cruz".into(),
        trade_name: String::new(),
        uploads: vec![Upload { kind: DocumentKind::TinId, filename: "id.png".into(), bytes: tin_card() }],
    };
    let claim_id = claims::submit(&db, &state.documents, &state.config.document_key, account, claim).await.unwrap();
    ocr::queue_claim(&state, claim_id).await;

    let deadline = Instant::now() + Duration::from_secs(120);
    let reading = loop {
        let reading = ocr::for_claim(&db, claim_id).await.unwrap().expect("queued");
        if reading.status != "reading" || Instant::now() > deadline {
            break reading;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(reading.status, "read", "{:?}", reading.error);
    assert_eq!(reading.engine.as_deref(), Some("paddleocr"));
    let fields = reading.fields.unwrap().0;
    assert_eq!(fields.tin.as_deref(), Some("123-456-789-000"));
    let verdicts: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT tin_match, name_match FROM document_readings").fetch_one(&db).await.unwrap();
    assert_eq!(verdicts, (Some("match".into()), Some("match".into())));

    // Deciding the claim purges the file, and the reading goes with it.
    let reviewer: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('staff@example.test') RETURNING id")
            .fetch_one(&db)
            .await
            .unwrap();
    claims::decide(&db, &state.documents, claim_id, reviewer, &claims::Decision::Approve, "").await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM document_readings").fetch_one(&db).await.unwrap();
    assert_eq!(left, 0);
}
