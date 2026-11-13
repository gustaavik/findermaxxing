//! The real models, downloaded from Hugging Face (~1.6 GB the first time):
//!
//! ```sh
//! FM_MODELS_DIR=/somewhere/models cargo test --release -p adapter-candle-embeddings --test real_models -- --ignored --nocapture
//! ```
//!
//! Checks that they load, that queries in one language find documents in
//! another, and that a text query finds the right photo. Similarities are
//! printed, to tune the relevance floors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, reason = "test code")]

use std::path::PathBuf;
use std::sync::Arc;

use adapter_candle_embeddings::{HuggingFaceInstaller, LocalImageEmbedder, LocalTextEmbedder};
use application::CancelToken;
use application::ports::{ImageEmbedder, LocalFile, ModelInstaller, ModelPack, TextEmbedder};

fn models_dir() -> PathBuf {
    std::env::var_os("FM_MODELS_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("models"), PathBuf::from)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

async fn install(pack: ModelPack) {
    let installer = HuggingFaceInstaller::new(models_dir(), tokio::runtime::Handle::current());
    if !installer.installed(pack) {
        installer.install(pack, Arc::new(|_| {}), CancelToken::never()).await.expect("download from huggingface.co");
    }
    assert!(installer.installed(pack));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "downloads ~490 MB of model weights"]
async fn text_queries_find_documents_across_languages() {
    install(ModelPack::Text).await;
    let embedder = LocalTextEmbedder::new(&models_dir(), tokio::runtime::Handle::current());
    let documents = [
        "Faktura for elektricitet. Beløb: 1.200 kr, betales senest 15. marts.",
        "Lasagne: lag pasta, tomatsauce med basilikum og bechamel, bag i 45 minutter.",
        "Minutes of the Q3 budget meeting: marketing spend cut by ten percent.",
        "Mietvertrag für die Wohnung in Berlin, Kaltmiete 900 Euro pro Monat.",
    ];
    let vectors = embedder.embed_passages(documents.iter().map(|d| (*d).to_owned()).collect()).await.unwrap();
    assert!(vectors.iter().all(|v| v.len() == 384 && (dot(v, v) - 1.0).abs() < 1e-3), "unit vectors of 384");
    let queries = [
        ("electricity invoice", 0),
        ("pasta recipe", 1),
        ("kvartalsbudget møde", 2),
        ("apartment rental contract", 3),
        ("strømregning", 0),
    ];
    for (query, expected) in queries {
        let q = embedder.embed_query(query).await.unwrap();
        let scores: Vec<f32> = vectors.iter().map(|v| dot(&q, v)).collect();
        println!("{query:30} {scores:?}");
        let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(best, expected, "{query}: {scores:?}");
        assert!(scores[expected] >= embedder.relevance_floor(), "{query} reaches the floor");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "downloads ~1.15 GB of model weights and two COCO photos"]
async fn text_queries_find_photos() {
    install(ModelPack::Images).await;
    let embedder = LocalImageEmbedder::new(&models_dir(), tokio::runtime::Handle::current());
    let dir = tempfile::tempdir().unwrap();
    // COCO val2017: two cats on a sofa; a bedroom; a kitchen with fruit.
    let photos = [("cats.jpg", "000000039769"), ("bedroom.jpg", "000000000632"), ("kitchen.jpg", "000000000139")];
    let mut vectors = Vec::new();
    for (name, id) in photos {
        let bytes = reqwest::get(format!("http://images.cocodataset.org/val2017/{id}.jpg"))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        let vector = embedder.embed_image(LocalFile::new(path)).await.unwrap().expect("a decodable photo");
        assert_eq!(vector.len(), 512);
        vectors.push(vector);
    }
    let not_an_image = dir.path().join("notes.jpg");
    std::fs::write(&not_an_image, b"not really a jpeg").unwrap();
    assert_eq!(embedder.embed_image(LocalFile::new(not_an_image)).await.unwrap(), None);
    for (query, expected) in
        [("two cats sleeping on a sofa", 0), ("to katte", 0), ("a bed in a bedroom", 1), ("soveværelse", 1)]
    {
        let q = embedder.embed_query(query).await.unwrap();
        let scores: Vec<f32> = vectors.iter().map(|v| dot(&q, v)).collect();
        println!("{query:30} {scores:?}");
        let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(best, expected, "{query}: {scores:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "downloads ~490 MB of model weights"]
async fn documents_about_the_same_thing_group_together() {
    install(ModelPack::Text).await;
    let embedder = LocalTextEmbedder::new(&models_dir(), tokio::runtime::Handle::current());
    let topics: [&[&str]; 3] = [
        &[
            "Faktura nr. 2041 for el-forbrug i marts. Beløb 1.200 kr, betales senest 15. april.",
            "Electricity invoice for April: 340 kWh, amount due 98.40 EUR by 20 May.",
            "Rechnung Stromverbrauch Mai, Betrag 87,10 Euro, fällig am 15. Juni.",
            "Your power bill for June. Total 112.60 EUR. Direct debit on 1 July.",
        ],
        &[
            "Lasagne: lag pasta, tomatsauce med basilikum og bechamel, bag i 45 minutter.",
            "Pancakes: mix flour, eggs and milk, rest 30 minutes, fry in butter.",
            "Kartoffelsuppe: Kartoffeln, Lauch und Brühe 20 Minuten kochen, pürieren.",
        ],
        &[
            "Booking confirmation: Hotel Arts Barcelona, 3 nights from 12 August, 2 adults.",
            "Flight SK 1415 Copenhagen to Barcelona, departs 12 Aug 07:35, seat 14C.",
            "Boardingkort: SK 1416 Barcelona–København, 15. august kl. 18.10.",
        ],
    ];
    let texts: Vec<String> = topics.iter().flat_map(|t| t.iter().map(|s| (*s).to_owned())).collect();
    let passages: Vec<String> = texts.iter().map(|t| domain::passages(t).remove(0)).collect();
    let vectors = embedder.embed_passages(passages).await.unwrap();
    let topic_of: Vec<usize> = topics.iter().enumerate().flat_map(|(i, t)| std::iter::repeat_n(i, t.len())).collect();
    let (mut within, mut across) = (Vec::new(), Vec::new());
    for i in 0..vectors.len() {
        for j in (i + 1)..vectors.len() {
            let similarity = dot(&vectors[i], &vectors[j]);
            if topic_of[i] == topic_of[j] { within.push(similarity) } else { across.push(similarity) }
        }
    }
    #[allow(clippy::cast_precision_loss, reason = "a few dozen pairs")]
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    println!("within: mean {:.3} min {:.3}", mean(&within), within.iter().copied().fold(f32::MAX, f32::min));
    println!("across: mean {:.3} max {:.3}", mean(&across), across.iter().copied().fold(f32::MIN, f32::max));
    let groups = domain::cluster(&vectors, embedder.grouping_threshold());
    println!("groups at {}: {groups:?}", embedder.grouping_threshold());
    assert_eq!(groups, [vec![0, 1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]]);
}
