//! Sweep `stopword_min_words` and `min_families` over a corpus sample to see
//! what each setting costs and buys, instead of picking a threshold by feel.
use ovis_prune::config::QualityConfig;
use ovis_prune::quality;

#[derive(serde::Deserialize)]
struct Doc { text: String }

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let docs: Vec<Doc> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    println!("{:>10} {:>10} {:>9} {:>8}", "stopword", "families", "flagged", "pct");
    for stopword_min in [50usize, 75, 100, 150] {
        for min_families in [2usize, 3] {
            let mut config = QualityConfig { min_families, ..QualityConfig::default() };
            config.stopword_min_words = stopword_min;
            let flagged = docs
                .iter()
                .filter(|d| {
                    let m = quality::measure(&d.text);
                    quality::is_candidate(&quality::evaluate(&m, &config), &config)
                })
                .count();
            println!(
                "{stopword_min:>10} {min_families:>10} {flagged:>9} {:>7.1}%",
                flagged as f64 / docs.len() as f64 * 100.0
            );
        }
    }
    Ok(())
}
