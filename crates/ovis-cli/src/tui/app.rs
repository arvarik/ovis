use crate::models::{ChunkRecord, DocumentRecord};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    LeftList,
    RightInspector,
    SearchInput,
}

pub struct App {
    pub all_documents: Vec<DocumentRecord>,
    pub filtered_documents: Vec<DocumentRecord>,
    pub selected_index: usize,
    pub active_pane: ActivePane,
    pub search_query: String,
    pub page: usize,
    pub per_page: usize,
    pub chunks_cache: HashMap<String, Vec<ChunkRecord>>,
    pub status_message: Option<String>,
    pub delete_confirm: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(documents: Vec<DocumentRecord>, chunks: Vec<ChunkRecord>) -> Self {
        let mut chunks_cache = HashMap::new();
        for chunk in chunks {
            chunks_cache
                .entry(chunk.document_id.clone())
                .or_insert_with(Vec::new)
                .push(chunk);
        }

        let mut app = Self {
            all_documents: documents.clone(),
            filtered_documents: documents,
            selected_index: 0,
            active_pane: ActivePane::LeftList,
            search_query: String::new(),
            page: 1,
            per_page: 8,
            chunks_cache,
            status_message: Some("Ready - Press '/' to search, [Tab] to switch pane, 'd' to delete, 'q' to quit".to_string()),
            delete_confirm: None,
            should_quit: false,
        };
        app.apply_filter();
        app
    }

    pub fn apply_filter(&mut self) {
        if self.search_query.trim().is_empty() {
            self.filtered_documents = self.all_documents.clone();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_documents = self
                .all_documents
                .iter()
                .filter(|doc| {
                    doc.id.to_lowercase().contains(&query)
                        || doc.semantic_id.to_lowercase().contains(&query)
                        || doc
                            .metadata
                            .get("source")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&query)
                })
                .cloned()
                .collect();
        }

        self.page = 1;
        self.selected_index = 0;
    }

    pub fn total_pages(&self) -> usize {
        if self.filtered_documents.is_empty() {
            1
        } else {
            (self.filtered_documents.len() + self.per_page - 1) / self.per_page
        }
    }

    pub fn current_page_documents(&self) -> &[DocumentRecord] {
        if self.filtered_documents.is_empty() {
            return &[];
        }
        let start = (self.page - 1) * self.per_page;
        if start >= self.filtered_documents.len() {
            return &[];
        }
        let end = (start + self.per_page).min(self.filtered_documents.len());
        &self.filtered_documents[start..end]
    }

    pub fn selected_document(&self) -> Option<&DocumentRecord> {
        let page_docs = self.current_page_documents();
        page_docs.get(self.selected_index)
    }

    pub fn selected_chunks(&self) -> &[ChunkRecord] {
        if let Some(doc) = self.selected_document() {
            self.chunks_cache.get(&doc.id).map(|v| v.as_slice()).unwrap_or(&[])
        } else {
            &[]
        }
    }

    pub fn select_next(&mut self) {
        let page_docs_count = self.current_page_documents().len();
        if page_docs_count > 0 && self.selected_index + 1 < page_docs_count {
            self.selected_index += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn next_page(&mut self) {
        if self.page < self.total_pages() {
            self.page += 1;
            self.selected_index = 0;
        }
    }

    pub fn prev_page(&mut self) {
        if self.page > 1 {
            self.page -= 1;
            self.selected_index = 0;
        }
    }

    pub fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::LeftList => ActivePane::RightInspector,
            ActivePane::RightInspector => ActivePane::LeftList,
            ActivePane::SearchInput => ActivePane::LeftList,
        };
    }

    pub fn delete_selected(&mut self) {
        if let Some(doc) = self.selected_document().cloned() {
            let doc_id = doc.id.clone();
            self.all_documents.retain(|d| d.id != doc_id);
            self.chunks_cache.remove(&doc_id);
            self.apply_filter();
            self.status_message = Some(format!("Successfully deleted page '{}'", doc_id));
        }
        self.delete_confirm = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_docs() -> Vec<DocumentRecord> {
        vec![
            DocumentRecord {
                id: "https://docs.onyx.app/web".to_string(),
                from_beginning: Some(true),
                semantic_id: "Web Connector Docs".to_string(),
                link: Some("https://docs.onyx.app/web".to_string()),
                doc_updated_at: None,
                primary_owners: Some(vec!["team@onyx.app".to_string()]),
                secondary_owners: None,
                metadata: json!({"source": "web"}),
            },
            DocumentRecord {
                id: "https://github.com/onyx-dot-app".to_string(),
                from_beginning: Some(true),
                semantic_id: "Onyx Repo".to_string(),
                link: Some("https://github.com/onyx-dot-app".to_string()),
                doc_updated_at: None,
                primary_owners: Some(vec!["devs@onyx.app".to_string()]),
                secondary_owners: None,
                metadata: json!({"source": "github"}),
            },
        ]
    }

    #[test]
    fn test_app_filter_and_selection() {
        let docs = sample_docs();
        let mut app = App::new(docs, vec![]);
        assert_eq!(app.filtered_documents.len(), 2);
        assert_eq!(app.selected_document().unwrap().semantic_id, "Web Connector Docs");

        app.search_query = "github".to_string();
        app.apply_filter();
        assert_eq!(app.filtered_documents.len(), 1);
        assert_eq!(app.selected_document().unwrap().semantic_id, "Onyx Repo");

        app.search_query.clear();
        app.apply_filter();
        assert_eq!(app.filtered_documents.len(), 2);
    }

    #[test]
    fn test_app_delete() {
        let docs = sample_docs();
        let mut app = App::new(docs, vec![]);
        app.delete_selected();
        assert_eq!(app.all_documents.len(), 1);
        assert_eq!(app.all_documents[0].semantic_id, "Onyx Repo");
    }
}
