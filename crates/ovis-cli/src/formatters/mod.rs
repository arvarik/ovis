use anyhow::Result;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, ContentArrangement, Table};
use crate::models::{ChunkRecord, ConnectorSummary, DocumentRecord};
use serde::Serialize;
use std::io::Write;

use crate::cli::OutputFormat;

#[derive(Debug, Serialize)]
pub struct DocumentListWrapper {
    pub total: usize,
    pub items: Vec<DocumentRecord>,
}

#[derive(Debug, Serialize)]
pub struct DocumentInspection {
    pub document: DocumentRecord,
    pub chunks: Vec<ChunkRecord>,
}

/// Formatter module providing output formatting in Table, JSON, YAML, and CSV formats.
pub struct Formatter;

impl Formatter {
    /// Formats and prints a document list according to the specified `OutputFormat`.
    pub fn print_documents(
        writer: &mut impl Write,
        documents: &[DocumentRecord],
        format: OutputFormat,
    ) -> Result<()> {
        match format {
            OutputFormat::Table => {
                let mut table = Table::new();
                table
                    .load_preset(UTF8_FULL)
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(vec![
                        Cell::new("DOCUMENT ID").fg(Color::Cyan),
                        Cell::new("TITLE / SEMANTIC ID").fg(Color::Cyan),
                        Cell::new("SOURCE").fg(Color::Cyan),
                        Cell::new("CHUNKS").fg(Color::Cyan),
                        Cell::new("LAST UPDATED").fg(Color::Cyan),
                    ]);

                for doc in documents {
                    let source = doc
                        .metadata
                        .get("source")
                        .and_then(|s| s.as_str())
                        .unwrap_or("web");

                    let chunks_count = doc
                        .metadata
                        .get("chunks")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(1);

                    let updated = doc
                        .doc_updated_at
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| "N/A".to_string());

                    table.add_row(vec![
                        Cell::new(&doc.id),
                        Cell::new(&doc.semantic_id),
                        Cell::new(source),
                        Cell::new(chunks_count),
                        Cell::new(updated),
                    ]);
                }

                writeln!(writer, "{}", table)?;
            }
            OutputFormat::Json => {
                let wrapper = DocumentListWrapper {
                    total: documents.len(),
                    items: documents.to_vec(),
                };
                let json = serde_json::to_string_pretty(&wrapper)?;
                writeln!(writer, "{}", json)?;
            }
            OutputFormat::Yaml => {
                let wrapper = DocumentListWrapper {
                    total: documents.len(),
                    items: documents.to_vec(),
                };
                let yaml = serde_yaml::to_string(&wrapper)?;
                writeln!(writer, "{}", yaml)?;
            }
            OutputFormat::Csv => {
                writeln!(writer, "id,semantic_id,source,updated_at")?;
                for doc in documents {
                    let source = doc
                        .metadata
                        .get("source")
                        .and_then(|s| s.as_str())
                        .unwrap_or("web");
                    let updated = doc
                        .doc_updated_at
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_default();
                    writeln!(writer, "\"{}\",\"{}\",\"{}\",\"{}\"", doc.id, doc.semantic_id, source, updated)?;
                }
            }
        }
        Ok(())
    }

    /// Formats and prints a single document inspection view.
    pub fn print_document_inspection(
        writer: &mut impl Write,
        doc: &DocumentRecord,
        chunks: &[ChunkRecord],
        raw: bool,
        format: OutputFormat,
    ) -> Result<()> {
        if raw {
            for chunk in chunks {
                writeln!(writer, "{}", chunk.content)?;
            }
            if chunks.is_empty() {
                writeln!(writer, "Semantic ID: {}", doc.semantic_id)?;
                writeln!(writer, "ID: {}", doc.id)?;
                if let Some(link) = &doc.link {
                    writeln!(writer, "Link: {}", link)?;
                }
            }
            return Ok(());
        }

        let inspection = DocumentInspection {
            document: doc.clone(),
            chunks: chunks.to_vec(),
        };

        match format {
            OutputFormat::Table => {
                let mut table = Table::new();
                table.load_preset(UTF8_FULL);

                table.add_row(vec![
                    Cell::new("Document ID").fg(Color::Yellow),
                    Cell::new(&doc.id),
                ]);
                table.add_row(vec![
                    Cell::new("Title / Semantic ID").fg(Color::Yellow),
                    Cell::new(&doc.semantic_id),
                ]);
                if let Some(link) = &doc.link {
                    table.add_row(vec![Cell::new("Link").fg(Color::Yellow), Cell::new(link)]);
                }
                if let Some(updated) = &doc.doc_updated_at {
                    table.add_row(vec![
                        Cell::new("Updated At").fg(Color::Yellow),
                        Cell::new(updated.format("%Y-%m-%d %H:%M:%S").to_string()),
                    ]);
                }
                if let Some(owners) = &doc.primary_owners {
                    table.add_row(vec![
                        Cell::new("Owners").fg(Color::Yellow),
                        Cell::new(owners.join(", ")),
                    ]);
                }
                table.add_row(vec![
                    Cell::new("Metadata").fg(Color::Yellow),
                    Cell::new(serde_json::to_string_pretty(&doc.metadata).unwrap_or_default()),
                ]);
                table.add_row(vec![
                    Cell::new("Total Chunks").fg(Color::Yellow),
                    Cell::new(chunks.len()),
                ]);

                writeln!(writer, "{}", table)?;

                if !chunks.is_empty() {
                    writeln!(writer, "\n--- Chunks Breakdown ---")?;
                    for chunk in chunks {
                        writeln!(
                            writer,
                            "Chunk #{}: ({} bytes)\n{}",
                            chunk.chunk_id,
                            chunk.content.len(),
                            chunk.content
                        )?;
                        writeln!(writer, "------------------------")?;
                    }
                }
            }
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(&inspection)?;
                writeln!(writer, "{}", json)?;
            }
            OutputFormat::Yaml => {
                let yaml = serde_yaml::to_string(&inspection)?;
                writeln!(writer, "{}", yaml)?;
            }
            OutputFormat::Csv => {
                writeln!(writer, "chunk_id,document_id,content_length")?;
                for chunk in chunks {
                    writeln!(writer, "{},\"{}\",{}", chunk.chunk_id, chunk.document_id, chunk.content.len())?;
                }
            }
        }
        Ok(())
    }

    /// Formats and prints connector summaries list.
    pub fn print_connectors(
        writer: &mut impl Write,
        connectors: &[ConnectorSummary],
        format: OutputFormat,
    ) -> Result<()> {
        match format {
            OutputFormat::Table => {
                let mut table = Table::new();
                table
                    .load_preset(UTF8_FULL)
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(vec![
                        Cell::new("ID").fg(Color::Green),
                        Cell::new("NAME").fg(Color::Green),
                        Cell::new("SOURCE").fg(Color::Green),
                        Cell::new("DISABLED").fg(Color::Green),
                        Cell::new("PAGES").fg(Color::Green),
                        Cell::new("LAST INDEXED").fg(Color::Green),
                    ]);

                for conn in connectors {
                    let last_indexed = conn
                        .last_indexed_at
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| "Never".to_string());

                    table.add_row(vec![
                        Cell::new(conn.connector_id),
                        Cell::new(&conn.connector_name),
                        Cell::new(&conn.connector_source),
                        Cell::new(if conn.disabled { "Yes" } else { "No" }),
                        Cell::new(conn.total_pages),
                        Cell::new(last_indexed),
                    ]);
                }

                writeln!(writer, "{}", table)?;
            }
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(&connectors)?;
                writeln!(writer, "{}", json)?;
            }
            OutputFormat::Yaml => {
                let yaml = serde_yaml::to_string(&connectors)?;
                writeln!(writer, "{}", yaml)?;
            }
            OutputFormat::Csv => {
                writeln!(writer, "id,name,source,disabled,total_pages")?;
                for conn in connectors {
                    writeln!(
                        writer,
                        "{},\"{}\",\"{}\",{},{}",
                        conn.connector_id, conn.connector_name, conn.connector_source, conn.disabled, conn.total_pages
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_doc() -> DocumentRecord {
        DocumentRecord {
            id: "https://docs.onyx.app/web".to_string(),
            from_beginning: Some(true),
            semantic_id: "Web Docs".to_string(),
            link: Some("https://docs.onyx.app/web".to_string()),
            doc_updated_at: None,
            primary_owners: Some(vec!["team@onyx.app".to_string()]),
            secondary_owners: None,
            metadata: json!({"source": "web", "chunks": 4}),
        }
    }

    #[test]
    fn test_print_documents_json() {
        let docs = vec![sample_doc()];
        let mut buf = Vec::new();
        Formatter::print_documents(&mut buf, &docs, OutputFormat::Json).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"total\": 1"));
        assert!(output.contains("https://docs.onyx.app/web"));
    }

    #[test]
    fn test_print_documents_yaml() {
        let docs = vec![sample_doc()];
        let mut buf = Vec::new();
        Formatter::print_documents(&mut buf, &docs, OutputFormat::Yaml).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("total: 1"));
        assert!(output.contains("semantic_id: Web Docs"));
    }

    #[test]
    fn test_print_documents_table() {
        let docs = vec![sample_doc()];
        let mut buf = Vec::new();
        Formatter::print_documents(&mut buf, &docs, OutputFormat::Table).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("DOCUMENT ID"));
        assert!(output.contains("https://docs.onyx.app/web"));
    }
}
