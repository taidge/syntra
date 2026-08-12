use std::path::Path;
use std::sync::Mutex;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

use contrix_core::error::AppError;

pub struct SearchService {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter<TantivyDocument>>,
    #[allow(dead_code)]
    schema: Schema,
    id_field: Field,
    name_field: Field,
    content_field: Field,
    doc_type_field: Field,
    owner_id_field: Field,
}

impl SearchService {
    pub fn new(index_path: &str) -> Result<Self, AppError> {
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let name_field = schema_builder.add_text_field("name", TEXT | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT);
        let doc_type_field = schema_builder.add_text_field("document_type", STRING | STORED);
        let owner_id_field = schema_builder.add_text_field("owner_id", STRING | STORED);
        let schema = schema_builder.build();

        let dir = Path::new(index_path);
        std::fs::create_dir_all(dir).map_err(|e| AppError::Storage(e.to_string()))?;

        let index = match Index::open_in_dir(dir) {
            Ok(existing) => existing,
            Err(_) => Index::create_in_dir(dir, schema.clone())
                .map_err(|e| AppError::Internal(e.to_string()))?,
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| AppError::Internal(format!("{e}")))?;

        let writer: IndexWriter<TantivyDocument> = index
            .writer(50_000_000)
            .map_err(|e| AppError::Internal(e.to_string()))?;

        Ok(Self {
            index,
            reader,
            writer: Mutex::new(writer),
            schema,
            id_field,
            name_field,
            content_field,
            doc_type_field,
            owner_id_field,
        })
    }

    pub fn index_document(
        &self,
        id: &str,
        name: &str,
        content: &str,
        doc_type: &str,
        owner_id: &str,
    ) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| AppError::Internal(format!("writer lock poisoned: {e}")))?;

        let id_term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(id_term);

        let mut doc = TantivyDocument::new();
        doc.add_text(self.id_field, id);
        doc.add_text(self.name_field, name);
        doc.add_text(self.content_field, content);
        doc.add_text(self.doc_type_field, doc_type);
        doc.add_text(self.owner_id_field, owner_id);
        writer
            .add_document(doc)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| AppError::Internal(e.to_string()))?;

        self.reader
            .reload()
            .map_err(|e| AppError::Internal(format!("reader reload: {e}")))?;

        Ok(())
    }

    pub fn remove_document(&self, id: &str) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| AppError::Internal(format!("writer lock poisoned: {e}")))?;
        let id_term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(id_term);
        writer
            .commit()
            .map_err(|e| AppError::Internal(e.to_string()))?;

        self.reader
            .reload()
            .map_err(|e| AppError::Internal(format!("reader reload: {e}")))?;

        Ok(())
    }

    pub fn search(
        &self,
        query: &str,
        owner_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, String, String, f32)>, AppError> {
        self.reader
            .reload()
            .map_err(|e| AppError::Internal(format!("reader reload: {e}")))?;
        let searcher = self.reader.searcher();
        let query_parser =
            QueryParser::for_index(&self.index, vec![self.name_field, self.content_field]);
        let parsed = query_parser
            .parse_query(query)
            .map_err(|e| AppError::Validation(e.to_string()))?;
        let top_docs = searcher
            .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let mut results = Vec::new();
        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = doc
                .get_first(self.name_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let doc_type = doc
                .get_first(self.doc_type_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let doc_owner = doc
                .get_first(self.owner_id_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if let Some(oid) = owner_id
                && doc_owner != oid
            {
                continue;
            }
            results.push((id, name, doc_type, score));
        }
        Ok(results)
    }
}
