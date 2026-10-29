//! A synchronous Spotlight query through CoreServices' `MDQuery` C API.
//!
//! `kMDQuerySynchronous` blocks until the initial gather finishes, which for a few user
//! folders is tens of milliseconds; callers run it off the main thread. Each call creates its
//! own query, so concurrent searches don't share state.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use objc2_core_foundation::{
    CFArray, CFDate, CFRetained, CFString, CFType, kCFAbsoluteTimeIntervalSince1970,
};

use crate::error::CoreError;

/// One raw match, before filtering and ranking.
pub(super) struct Hit {
    pub path: String,
    pub name: String,
    pub content_type: String,
    /// Seconds since 1970; 0 when Spotlight has no date.
    pub modified: f64,
}

type MDQueryRef = *mut c_void;
type MDItemRef = *const c_void;

const MD_QUERY_SYNCHRONOUS: usize = 1;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn MDQueryCreate(
        allocator: *const c_void,
        query_string: &CFString,
        value_list_attrs: *const c_void,
        sorting_attrs: *const c_void,
    ) -> MDQueryRef;
    fn MDQuerySetSearchScope(query: MDQueryRef, scope_directories: *const c_void, options: u32);
    fn MDQuerySetMaxCount(query: MDQueryRef, size: isize);
    fn MDQueryExecute(query: MDQueryRef, option_flags: usize) -> u8;
    fn MDQueryGetResultCount(query: MDQueryRef) -> isize;
    fn MDQueryGetResultAtIndex(query: MDQueryRef, index: isize) -> MDItemRef;
    fn MDItemCopyAttribute(item: MDItemRef, name: &CFString) -> *mut CFType;

    static kMDItemPath: &'static CFString;
    static kMDItemDisplayName: &'static CFString;
    static kMDItemContentType: &'static CFString;
    static kMDItemContentModificationDate: &'static CFString;
    static kMDItemFSContentChangeDate: &'static CFString;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// Releases the query however `run` returns.
struct Query(MDQueryRef);

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: created by `MDQueryCreate` (+1) and released exactly once.
        unsafe { CFRelease(self.0) };
    }
}

/// Runs `query_string` within `scopes` (existing folder paths), returning at most `max`
/// matches in Spotlight's order.
pub(super) fn run(
    query_string: &str,
    scopes: &[String],
    max: usize,
) -> Result<Vec<Hit>, CoreError> {
    let query_cf = CFString::from_str(query_string);
    // SAFETY: a valid query string; the null arguments are documented as optional.
    let raw = unsafe { MDQueryCreate(ptr::null(), &query_cf, ptr::null(), ptr::null()) };
    if raw.is_null() {
        return Err(CoreError::Other {
            message: "Spotlight rejected the search".into(),
        });
    }
    let query = Query(raw);

    let scope_strings: Vec<CFRetained<CFString>> =
        scopes.iter().map(|s| CFString::from_str(s)).collect();
    let scope_refs: Vec<&CFString> = scope_strings.iter().map(|s| &**s).collect();
    let scope_array = CFArray::from_objects(&scope_refs);
    let scope_ptr: *const CFArray<CFString> = &*scope_array;
    // SAFETY: `query` is live; the array of path strings is retained by the query.
    unsafe {
        MDQuerySetSearchScope(query.0, scope_ptr.cast(), 0);
        MDQuerySetMaxCount(query.0, max as isize);
    }

    // SAFETY: `query` is live and fully configured.
    if unsafe { MDQueryExecute(query.0, MD_QUERY_SYNCHRONOUS) } == 0 {
        return Err(CoreError::Other {
            message: "Spotlight search failed to run".into(),
        });
    }

    // SAFETY: `query` is live; the count is only read after the synchronous gather.
    let count = unsafe { MDQueryGetResultCount(query.0) }.max(0);
    let mut hits = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: `index` is within the result count; the item is owned by the query.
        let item = unsafe { MDQueryGetResultAtIndex(query.0, index) };
        if item.is_null() {
            continue;
        }
        if let Some(hit) = read_hit(item) {
            hits.push(hit);
        }
    }
    Ok(hits)
}

fn read_hit(item: MDItemRef) -> Option<Hit> {
    // SAFETY: the attribute names are immutable CoreServices constants.
    let (path_key, name_key, type_key, modified_key, changed_key) = unsafe {
        (
            kMDItemPath,
            kMDItemDisplayName,
            kMDItemContentType,
            kMDItemContentModificationDate,
            kMDItemFSContentChangeDate,
        )
    };
    let path = string_attribute(item, path_key)?;
    let name = string_attribute(item, name_key)
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_owned());
    let content_type = string_attribute(item, type_key).unwrap_or_default();
    let modified = date_attribute(item, modified_key)
        .or_else(|| date_attribute(item, changed_key))
        .unwrap_or(0.0);
    Some(Hit {
        path,
        name,
        content_type,
        modified,
    })
}

fn copy_attribute(item: MDItemRef, name: &CFString) -> Option<CFRetained<CFType>> {
    // SAFETY: `item` belongs to a live query; the Copy rule makes the result ours (+1).
    let raw = unsafe { MDItemCopyAttribute(item, name) };
    NonNull::new(raw).map(|ptr| unsafe { CFRetained::from_raw(ptr) })
}

fn string_attribute(item: MDItemRef, name: &CFString) -> Option<String> {
    let value = copy_attribute(item, name)?.downcast::<CFString>().ok()?;
    Some(value.to_string())
}

fn date_attribute(item: MDItemRef, name: &CFString) -> Option<f64> {
    let value = copy_attribute(item, name)?.downcast::<CFDate>().ok()?;
    // SAFETY: an immutable CoreFoundation constant.
    let epoch_offset = unsafe { kCFAbsoluteTimeIntervalSince1970 };
    Some(value.absolute_time() + epoch_offset)
}
