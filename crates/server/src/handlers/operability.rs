use salvo::prelude::*;

/// Machine-readable API contract. It intentionally describes the stable
/// public surface and links operators to the authenticated metrics endpoint.
#[handler]
pub async fn openapi(res: &mut Response) {
    res.status_code(StatusCode::OK);
    res.render(Json(openapi_document()));
}

fn openapi_document() -> serde_json::Value {
    let mut document = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Contrix Office API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Document storage, collaborative editing, conversion and administration API"
        },
        "servers": [{"url":"/api/v1"}],
        "components": {
            "securitySchemes": {"bearerAuth":{"type":"http","scheme":"bearer","bearerFormat":"JWT"}},
            "schemas": {
                "ApiError":{"type":"object","properties":{"success":{"const":false},"error":{"type":"object"}}},
                "Document":{"type":"object","required":["id","name","doc_type"],"properties":{"id":{"type":"string","format":"uuid"},"name":{"type":"string"},"doc_type":{"enum":["writer","calc","slides","whiteboard"]},"content":{}}},
                "ConversionJob":{"type":"object","properties":{"id":{"type":"string","format":"uuid"},"status":{"enum":["pending","processing","completed","failed","cancelled"]},"target_format":{"type":"string"}}}
            }
        },
        "security": [{"bearerAuth":[]}],
        "paths": {
            "/health":{"get":{"security":[],"summary":"Health check","responses":{"200":{"description":"Healthy"}}}},
            "/capabilities":{"get":{"security":[],"summary":"Server capabilities","responses":{"200":{"description":"Capabilities"}}}},
            "/auth/register":{"post":{"security":[],"summary":"Register","responses":{"201":{"description":"Registered"}}}},
            "/auth/login":{"post":{"security":[],"summary":"Authenticate","responses":{"200":{"description":"Authenticated"}}}},
            "/files":{"get":{"summary":"List files","responses":{"200":{"description":"Files"}}}},
            "/files/upload":{"post":{"summary":"Upload file","responses":{"201":{"description":"Uploaded"}}}},
            "/files/{id}":{"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],"get":{"summary":"Get file","responses":{"200":{"description":"File"}}},"put":{"summary":"Update file","responses":{"200":{"description":"Updated"}}},"delete":{"summary":"Trash file","responses":{"200":{"description":"Trashed"}}}},
            "/documents":{"post":{"summary":"Create document","responses":{"201":{"description":"Created"}}}},
            "/documents/{id}":{"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],"get":{"summary":"Get document","responses":{"200":{"description":"Document"}}},"put":{"summary":"Save document","responses":{"200":{"description":"Saved"}}}},
            "/documents/{id}/export/{format}":{"post":{"summary":"Export document","responses":{"200":{"description":"Export payload"}}}},
            "/convert":{"get":{"summary":"List durable conversion jobs","responses":{"200":{"description":"Jobs"}}},"post":{"summary":"Enqueue conversion","responses":{"201":{"description":"Queued"}}}},
            "/convert/{id}":{"get":{"summary":"Get conversion status","responses":{"200":{"description":"Job"}}}},
            "/convert/{id}/retry":{"post":{"summary":"Retry failed conversion","responses":{"202":{"description":"Requeued"}}}},
            "/webhooks":{"get":{"summary":"List webhooks (admin)","responses":{"200":{"description":"Webhooks"}}},"post":{"summary":"Create webhook (admin)","responses":{"201":{"description":"Created"}}}},
            "/webhooks/{id}/deliveries/{delivery_id}/retry":{"post":{"summary":"Retry failed webhook delivery (admin)","responses":{"202":{"description":"Requeued"}}}},
            "/admin/monitoring":{"get":{"summary":"Operational summary (admin)","responses":{"200":{"description":"Metrics summary"}}}},
            "/admin/monitoring/metrics":{"get":{"summary":"Prometheus exposition (admin)","responses":{"200":{"description":"Prometheus text"}}}},
            "/ws/collab/{document_id}":{"get":{"summary":"Collaborative editing WebSocket","responses":{"101":{"description":"Switching protocols"}}}}
        }
    });
    if let Some(paths) = document
        .get_mut("paths")
        .and_then(|paths| paths.as_object_mut())
    {
        for (path, item) in paths {
            let parameters = path.split('/').filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
                .map(|name| serde_json::json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}))
                .collect::<Vec<_>>();
            if !parameters.is_empty()
                && let Some(item) = item.as_object_mut()
            {
                item.insert("parameters".into(), serde_json::Value::Array(parameters));
            }
        }
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_declares_all_template_parameters_and_real_methods() {
        let document = openapi_document();
        assert_eq!(document["openapi"], "3.1.0");
        assert!(document["paths"]["/documents"].get("get").is_none());
        assert_eq!(
            document["paths"]["/documents/{id}/export/{format}"]["parameters"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            document["paths"]["/webhooks/{id}/deliveries/{delivery_id}/retry"]["post"].is_object()
        );
    }
}
