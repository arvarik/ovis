//! Print the flagged/unflagged split with a text preview, to eyeball whether
//! the gates are catching junk or catching content.
use ovis_prune::config::QualityConfig;
use ovis_prune::quality;

#[derive(serde::Deserialize)]
struct Doc {
    id: String,
    text: String,
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let want_flagged = std::env::args()
        .nth(2)
        .map(|s| s == "flagged")
        .unwrap_or(true);
    let docs: Vec<Doc> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let config = QualityConfig::default();
    let mut shown = 0;
    for doc in &docs {
        let m = quality::measure(&doc.text);
        let f = quality::evaluate(&m, &config);
        let flagged = quality::is_candidate(&f, &config);
        if flagged != want_flagged || shown >= 12 {
            continue;
        }
        shown += 1;
        let preview: String = doc.text.chars().take(110).collect();
        println!(
            "[{} gates/{} fam] {}\n    gates: {:?}\n    words={} txt={:?}\n",
            f.len(),
            quality::families_failed(&f),
            doc.id,
            f.iter().map(|g| g.code()).collect::<Vec<_>>(),
            m.word_count,
            preview.replace('\n', "⏎")
        );
    }
    Ok(())
}
