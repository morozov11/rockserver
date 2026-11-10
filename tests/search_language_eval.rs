//! Opt-in local E5 evaluation of the 28 semantic language prototypes.

#![cfg(feature = "onnx-local")]

use std::sync::Arc;

use rockserver::providers::onnx_e5::{OnnxE5Config, OnnxE5EmbeddingProvider};
use rockserver::search::{EmbeddingProvider, SemanticLanguageClassifier};

const EXPLICIT: &[(&str, &str)] = &[
    ("ar", "play jazz in Arabic language"),
    ("cs", "play jazz in Czech language"),
    ("da", "play jazz in Danish language"),
    ("de", "play jazz in German language"),
    ("el", "play jazz in Greek language"),
    ("en", "play jazz in English language"),
    ("es", "play jazz in Spanish language"),
    ("fi", "play jazz in Finnish language"),
    ("fr", "play jazz in French language"),
    ("he", "play jazz in Hebrew language"),
    ("hi", "play jazz in Hindi language"),
    ("hu", "play jazz in Hungarian language"),
    ("id", "play jazz in Indonesian language"),
    ("it", "play jazz in Italian language"),
    ("ja", "play jazz in Japanese language"),
    ("ko", "play jazz in Korean language"),
    ("nl", "play jazz in Dutch language"),
    ("no", "play jazz in Norwegian language"),
    ("pl", "play jazz in Polish language"),
    ("pt", "play jazz in Portuguese language"),
    ("ro", "play jazz in Romanian language"),
    ("ru", "play jazz in Russian language"),
    ("sv", "play jazz in Swedish language"),
    ("th", "play jazz in Thai language"),
    ("tr", "play jazz in Turkish language"),
    ("uk", "play jazz in Ukrainian language"),
    ("vi", "play jazz in Vietnamese language"),
    ("zh", "play jazz in Chinese language"),
];

const NO_LANGUAGE: &[&str] = &[
    "play jazz",
    "включи джаз",
    "play rock music",
    "show ambient stations",
    "find a calm radio station",
    "energetic rock for a workout",
    "gentle rock for relaxing",
    "rock songs performed by other artists",
    "underground eighties synth music",
    "SomaFM Metal Detector",
    "Rock Antenne Soft Rock",
    "Radio Record Rock",
    "play Radio Paradise Rock Mix",
    "найди Радио Рекорд рок",
    "поставь Рок Антенне софт рок",
    "play music from France",
    "включи джаз из Франции",
    "how to bake sourdough bread",
    "weather forecast tomorrow",
    "repair a bicycle chain",
];

/// Prints classifier confusion counts using pinned local E5 assets without network calls.
#[tokio::test]
#[ignore = "requires pinned ROCKSERVER_ONNX_MODEL_PATH, ROCKSERVER_ONNX_TOKENIZER_PATH and ORT_DYLIB_PATH"]
async fn measures_language_prototypes_on_explicit_and_unmarked_requests() {
    let config = OnnxE5Config::from_env().expect("pinned local E5 assets");
    let provider = Arc::new(OnnxE5EmbeddingProvider::load(&config).unwrap());
    let classifier = SemanticLanguageClassifier::load(provider.clone())
        .await
        .unwrap();
    let mut correct = 0;
    let mut wrong = 0;
    let mut missed = 0;
    for &(expected, text) in EXPLICIT {
        let embedding = provider.embed(text).await.unwrap();
        let result = classifier.classify(&embedding);
        correct += usize::from(
            result
                .as_ref()
                .is_some_and(|match_| match_.code == expected),
        );
        wrong += usize::from(
            result
                .as_ref()
                .is_some_and(|match_| match_.code != expected),
        );
        missed += usize::from(result.is_none());
        println!("explicit expected={expected} query={text:?} result={result:?}");
    }
    let mut false_positive = 0;
    for &text in NO_LANGUAGE {
        let embedding = provider.embed(text).await.unwrap();
        let result = classifier.classify(&embedding);
        false_positive += usize::from(result.is_some());
        println!("unmarked query={text:?} result={result:?}");
    }
    println!(
        "language_eval prototypes={} explicit_correct={correct} wrong={wrong} missed={missed} unmarked={} false_positive={false_positive}",
        EXPLICIT.len(),
        NO_LANGUAGE.len()
    );
    assert_eq!(EXPLICIT.len(), 28);
}
