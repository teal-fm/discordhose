// parts rewritten from https://github.com/mary-ext/atcute/blob/trunk/packages/oauth/browser-client/
// from https://github.com/espeon/geranium/blob/main/src/resolve.rs
// MIT License

use lazy_static::lazy_static;
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// Cache for handle resolution - maps handle to DID
type HandleCache = Cache<String, String>;

// Cache for DID documents - maps DID to DidDocument
type DidDocumentCache = Cache<String, DidDocument>;

// Global cache instances
lazy_static::lazy_static! {
    static ref HANDLE_CACHE: HandleCache = Cache::builder()
        .time_to_live(Duration::from_secs(3600)) // 1 hour TTL
        .max_capacity(10000)
        .build();
    
    static ref DID_DOCUMENT_CACHE: DidDocumentCache = Cache::builder()
        .time_to_live(Duration::from_secs(3600)) // 1 hour TTL
        .max_capacity(10000)
        .build();
}

// should be same as regex /^did:[a-z]+:[\S\s]+/
fn is_did(did: &str) -> bool {
    let parts: Vec<&str> = did.split(':').collect();

    if parts.len() != 3 {
        // must have exactly 3 parts: "did", method, and identifier
        return false;
    }

    if parts[0] != "did" {
        // first part must be "did"
        return false;
    }

    if !parts[1].chars().all(|c| c.is_ascii_lowercase()) {
        // method must be all lowercase
        return false;
    }

    if parts[2].is_empty() {
        // identifier can't be empty
        return false;
    }

    true
}

fn is_valid_domain(domain: &str) -> bool {
    // Check if empty or too long
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }

    // Split into labels
    let labels: Vec<&str> = domain.split('.').collect();

    // Must have at least 2 labels
    if labels.len() < 2 {
        return false;
    }

    // Check each label
    for label in labels {
        // Label length check
        if label.is_empty() || label.len() > 63 {
            return false;
        }

        // Must not start or end with hyphen
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }

        // Check characters
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }

    true
}

async fn resolve_handle(handle: &str, resolver_app_view: &str) -> Result<String, reqwest::Error> {
    // Check cache first
    if let Some(cached_did) = HANDLE_CACHE.get(handle).await {
        println!("🎯 Cache HIT for handle: {} -> {}", handle, cached_did);
        return Ok(cached_did);
    }

    println!("❌ Cache MISS for handle: {}, resolving from API", handle);

    // If not in cache, resolve from API
    let res = reqwest::get(format!(
        "{}/xrpc/com.atproto.identity.resolveHandle?handle={}",
        resolver_app_view, handle
    ))
    .await?
    .json::<ResolvedHandle>()
    .await?;

    let did = res.did;
    
    // Cache the result
    HANDLE_CACHE.insert(handle.to_string(), did.clone()).await;
    println!("💾 Cached handle resolution: {} -> {}", handle, did);
    
    Ok(did)
}

async fn get_did_doc(did: &str) -> Result<DidDocument, reqwest::Error> {
    // Check cache first
    if let Some(cached_doc) = DID_DOCUMENT_CACHE.get(did).await {
        println!("🎯 Cache HIT for DID document: {}", did);
        return Ok(cached_doc);
    }

    println!("❌ Cache MISS for DID document: {}, resolving from API", did);

    // If not in cache, resolve from API
    // get the specific did spec
    // did:plc:abcd1e -> plc
    let parts: Vec<&str> = did.split(':').collect();
    let spec = parts[1];
    let doc = match spec {
        "plc" => {
            println!("📡 Fetching DID document from PLC directory for: {}", did);
            let res: DidDocument = reqwest::get(format!("https://plc.directory/{}", did))
                .await?
                .error_for_status()?
                .json()
                .await?;
            res
        }
        "web" => {
            if !is_valid_domain(parts[2]) {
                todo!("Error for domain in did:web is not valid");
            };
            let ident = parts[2];
            println!("📡 Fetching DID document from web domain: {}", ident);
            let res = reqwest::get(format!("https://{}/.well-known/did.json", ident))
                .await?
                .error_for_status()?
                .json()
                .await?;
            res
        }
        _ => todo!("Identifier not supported"),
    };

    // Cache the result
    DID_DOCUMENT_CACHE.insert(did.to_string(), doc.clone()).await;
    println!("💾 Cached DID document: {}", did);
    
    Ok(doc)
}

fn get_pds_endpoint(doc: &DidDocument) -> Option<DidDocumentService> {
    get_service_endpoint(doc, "#atproto_pds", "AtprotoPersonalDataServer")
}

fn get_service_endpoint(
    doc: &DidDocument,
    svc_id: &str,
    svc_type: &str,
) -> Option<DidDocumentService> {
    doc.service
        .iter()
        .find(|svc| svc.id == svc_id && svc._type == svc_type)
        .cloned()
}

fn extract_handle_from_doc(doc: &DidDocument) -> Option<String> {
    // Look through alsoKnownAs list for at:// URLs
    for also_known_as in &doc.also_known_as {
        if also_known_as.starts_with("at://") {
            // Extract handle from "at://handle.domain" format
            let handle = also_known_as.strip_prefix("at://")?;
            println!("🎯 Found handle in alsoKnownAs: {} -> {}", also_known_as, handle);
            return Some(handle.to_string());
        }
    }
    None
}

pub async fn resolve_identity(
    id: &str,
    resolver_app_view: &str,
) -> Result<ResolvedIdentity, reqwest::Error> {
    println!("🔍 Resolving identity: {}", id);
    
    // is our identifier a did
    let did = if is_did(id) {
        println!("✅ Input is already a DID: {}", id);
        id
    } else {
        println!("🔗 Input is a handle, resolving to DID: {}", id);
        // our id must be either invalid or a handle
        if let Ok(res) = resolve_handle(id, resolver_app_view).await {
            &res.clone()
        } else {
            todo!("Error type for could not resolve handle")
        }
    };

    let doc = get_did_doc(did).await?;
    let pds = get_pds_endpoint(&doc);

    if pds.is_none() {
        todo!("Error for could not find PDS")
    }

    // Extract handle from alsoKnownAs list
    let handle = extract_handle_from_doc(&doc).unwrap_or_else(|| {
        println!("⚠️  No handle found in alsoKnownAs, using original input: {}", id);
        id.to_string()
    });

    println!("✅ Successfully resolved identity: {} -> {} (handle: {}) (PDS: {})", 
             id, did, handle, pds.as_ref().unwrap().service_endpoint);

    return Ok(ResolvedIdentity {
        did: did.to_owned(),
        doc,
        identity: handle,
        pds: pds.unwrap().service_endpoint,
    });
}

/// Clear all cached handle resolutions and DID documents
pub async fn clear_cache() {
    HANDLE_CACHE.invalidate_all();
    DID_DOCUMENT_CACHE.invalidate_all();
}

/// Get cache statistics for monitoring
pub async fn get_cache_stats() -> (u64, u64) {
    let handle_count = HANDLE_CACHE.entry_count();
    let did_doc_count = DID_DOCUMENT_CACHE.entry_count();
    (handle_count, did_doc_count)
}

// want this to be reusable on case of scope expansion :(
#[allow(dead_code)]
#[derive(Serialize, Deserialize, Debug)]
pub struct ResolvedIdentity {
    pub did: String,
    pub doc: DidDocument,
    pub identity: String,
    // should prob be url type but not really needed rn
    pub pds: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ResolvedHandle {
    did: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DidDocument {
    #[serde(alias = "@context")]
    pub _context: Vec<String>,
    pub id: String,
    #[serde(alias = "alsoKnownAs")]
    pub also_known_as: Vec<String>,
    #[serde(alias = "verificationMethod")]
    pub verification_method: Vec<DidDocumentVerificationMethod>,
    pub service: Vec<DidDocumentService>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DidDocumentVerificationMethod {
    pub id: String,
    #[serde(alias = "type")]
    pub _type: String,
    pub controller: String,
    #[serde(alias = "publicKeyMultibase")]
    pub public_key_multibase: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DidDocumentService {
    pub id: String,
    #[serde(alias = "type")]
    pub _type: String,
    #[serde(alias = "serviceEndpoint")]
    pub service_endpoint: String,
}
