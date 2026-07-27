//! The per-invocation context every command receives.

use crate::api::ApiClient;
use crate::cli::GlobalArgs;
use crate::config::Resolved;
use crate::error::CliResult;
use crate::output::{ColorChoice, Format, Out};
use crate::prompt::Interaction;

pub struct Ctx {
    pub api: ApiClient,
    pub out: Out,
    pub interaction: Interaction,
    pub cfg: Resolved,
}

impl Ctx {
    pub fn build(globals: &GlobalArgs) -> CliResult<Self> {
        let cfg = crate::config::resolve(&crate::config::Overrides {
            server: globals.server.clone(),
            token: globals.token.clone(),
            color: globals.color.map(|c| c.to_string()),
            profile: globals.profile.clone(),
        })?;

        let choice: ColorChoice = cfg
            .color
            .value
            .parse()
            .map_err(crate::error::CliError::Usage)?;

        let mut out = Out::new(
            globals.format.unwrap_or(Format::Table),
            choice,
            globals.quiet,
        );
        out.max_width = cfg.table_max_width;
        out.pager = cfg.pager.as_ref().map(|p| p.value.clone());
        out.wide = globals.wide;
        out.columns = globals.columns.clone();
        out.no_headers = globals.no_headers;
        out.hints = cfg.hints;

        let api = ApiClient::new(
            &cfg.server.value,
            cfg.token.as_ref().map(|t| t.value.clone()),
            globals.verbose > 0,
        )?;

        Ok(Self {
            api,
            out,
            interaction: Interaction::new(globals.yes, globals.no_input),
            cfg,
        })
    }

    /// Relative timestamps are for reading, absolute ones for parsing. A
    /// terminal gets the former unless `--wide` asked for the full detail.
    pub fn relative_time(&self) -> bool {
        self.out.stdout_tty && !self.out.wide
    }
}

impl std::fmt::Display for ColorChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            ColorChoice::Auto => "auto",
            ColorChoice::Always => "always",
            ColorChoice::Never => "never",
        };
        f.write_str(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_choices_round_trip_through_their_words() {
        for choice in [ColorChoice::Auto, ColorChoice::Always, ColorChoice::Never] {
            let word = choice.to_string();
            assert_eq!(word.parse::<ColorChoice>().unwrap(), choice);
        }
    }
}
