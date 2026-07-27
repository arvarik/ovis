export interface PageListItem {
  id: string;
  semantic_id: string;
  link?: string;
  doc_updated_at?: string;
  connector_id?: number;
  connector_name?: string;
  connector_source?: string;
  chunk_count: number;
  metadata: Record<string, any>;
}

export interface ListPagesResponse {
  total: number;
  page: number;
  limit: number;
  total_pages: number;
  items: PageListItem[];
}

export interface PageChunkItem {
  chunk_id: number;
  content: string;
  token_count: number;
  embedding_dimension?: number;
  embedding_model?: string;
  embedding_sample?: number[];
  embeddings?: number[];
}

export interface PageDetailResponse {
  id: string;
  semantic_id: string;
  link?: string;
  doc_updated_at?: string;
  primary_owners?: string[];
  secondary_owners?: string[];
  connector_id?: number;
  connector_source?: string;
  connector_name?: string;
  metadata: Record<string, any>;
  full_text: string;
  chunks: PageChunkItem[];
}

export interface DeletePageResponse {
  success: boolean;
  deleted_doc_id: string;
  chunks_deleted: number;
}

export interface BatchDeleteResponse {
  success: boolean;
  total_deleted: number;
  total_chunks_deleted: number;
  deleted_ids: string[];
}

export interface ConnectorSummary {
  connector_id: number;
  connector_name: string;
  connector_source: string;
  disabled: boolean;
  total_pages: number;
  last_indexed_at?: string;
}

export interface PruneCandidatePair {
  id: string;
  doc_id_a: string;
  title_a: string;
  connector_a: string;
  doc_id_b: string;
  title_b: string;
  connector_b: string;
  similarity_score: number;
  shingle_overlap_percent: number;
  flag_reason: 'near_duplicate' | 'empty_stub' | 'boilerplate_error' | 'length_anomaly';
  created_at: string;
}
