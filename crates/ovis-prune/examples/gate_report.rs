//! Run the quality gates over a JSON array of `{id, text}` and report the
//! flag rate per gate. Used to sanity-check thresholds against a real corpus
//! sample before trusting them:
//!
//! ```text
//! cargo run -p ovis-prune --example gate_report -- sample_docs.json
//! ```

use std::collections::BTreeMap;

use ovis_prune::config::QualityConfig;
use ovis_prune::quality;

#[derive(serde::Deserialize)]
struct Doc {
    id: String,
    text: String,
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: gate_report <docs.json>");
    let docs: Vec<Doc> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let config = QualityConfig::default();

    let mut per_gate: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failure_histogram: BTreeMap<usize, usize> = BTreeMap::new();
    let mut examples: BTreeMap<&str, Vec<String>> = BTreeMap::new();

    for doc in &docs {
        let metrics = quality::measure(&doc.text);
        let failures = quality::evaluate(&metrics, &config);
        *failure_histogram.entry(failures.len()).or_default() += 1;
        for gate in &failures {
            *per_gate.entry(gate.code()).or_default() += 1;
            let bucket = examples.entry(gate.code()).or_default();
            if bucket.len() < 3 {
                bucket.push(doc.id.clone());
            }
        }
    }

    let total = docs.len();
    println!("documents: {total}\n");
    println!("failures per document:");
    for (count, docs) in &failure_histogram {
        println!(
            "  {count} gate(s): {docs:>5}  ({:.1}%)",
            *docs as f64 / total as f64 * 100.0
        );
    }
    let mut flagged = 0usize;
    let mut by_families: BTreeMap<usize, usize> = BTreeMap::new();
    for doc in &docs {
        let metrics = quality::measure(&doc.text);
        let failures = quality::evaluate(&metrics, &config);
        *by_families
            .entry(quality::families_failed(&failures))
            .or_default() += 1;
        if quality::is_candidate(&failures, &config) {
            flagged += 1;
        }
    }
    println!("\nfamilies spanned per document:");
    for (families, docs) in &by_families {
        println!(
            "  {families} famil(y/ies): {docs:>5}  ({:.1}%)",
            *docs as f64 / total as f64 * 100.0
        );
    }
    println!(
        "\ncandidates at min_failures={} min_families={}: {flagged} ({:.1}%)\n",
        config.min_failures,
        config.min_families,
        flagged as f64 / total as f64 * 100.0
    );

    println!("per-gate trip rate:");
    for (gate, count) in &per_gate {
        println!(
            "  {gate:<24} {count:>5}  ({:.1}%)  e.g. {}",
            *count as f64 / total as f64 * 100.0,
            examples[gate].first().map(String::as_str).unwrap_or("")
        );
    }
    Ok(())
}
