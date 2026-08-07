//! Probe a live endpoint: enumerate its models and report what each one
//! actually does.
//!
//! ```text
//! cargo run -p ovis-llm --example probe -- llamacpp http://192.168.4.240:8082
//! cargo run -p ovis-llm --example probe -- gemini "" $KEY gemini-2.5-flash-lite
//! ```
//!
//! Exists because the handshake's whole claim is that documentation and
//! catalogues are unreliable — that claim is only worth anything if it is easy
//! to check against a real endpoint.

use ovis_llm::{handshake, Provider, ProviderKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let kind = ProviderKind::parse(
        &args
            .next()
            .expect("usage: probe <kind> [url] [key] [model]"),
    )?;
    let url = args.next().filter(|s| !s.is_empty());
    let key = args.next().filter(|s| !s.is_empty());
    let only = args.next();

    let provider = Provider::new(kind, url.as_deref(), key)?;
    println!("endpoint: {}\n", provider.base_url());

    let models = provider.list_models().await?;
    println!("{} model(s) listed\n", models.len());

    for model in &models {
        if let Some(only) = &only {
            if &model.id != only {
                continue;
            }
        }
        if model.advertised.is_embedding {
            println!("  {:<34} (embedding — not a judge)", model.id);
            continue;
        }
        print!("  {:<34} ", model.id);
        match handshake::probe(&provider, &model.id).await {
            Ok(caps) => {
                println!("{}", caps.summary());
                if !caps.usable_as_judge() {
                    println!("      NOT USABLE AS A JUDGE");
                }
                for note in &caps.notes {
                    println!("      · {note}");
                }
            }
            Err(err) => println!("probe failed: {err}"),
        }
    }
    Ok(())
}
