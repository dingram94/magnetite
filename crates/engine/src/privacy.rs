//! Global Privacy Control: the signal that tells a website its visitor does
//! not want their data sold or shared. It is a request header (`Sec-GPC:
//! 1`) and a property of the page (`navigator.globalPrivacyControl`).
//! Chromium has neither, so both are added here: the header on the way out,
//! the property by the script a page runs before its own (`scriptlets.rs`).

use std::sync::atomic::{AtomicBool, Ordering};

use cef::*;

/// Whether the signal is sent.
static ON: AtomicBool = AtomicBool::new(false);

/// Send the signal from now on, or stop. Pages that are open get the
/// header with their next request and the property with their next
/// document.
pub fn set_global_privacy_control(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// What a web page runs before its own scripts while the signal is on:
/// `navigator.globalPrivacyControl` is true, as the proposal has it.
pub(crate) const PAGE_SCRIPT: &str = "try{Object.defineProperty(Navigator.prototype,'globalPrivacyControl',{get(){return true},configurable:true,enumerable:true})}catch(e){}";

/// `handler`, or none, with the header added to the request first while
/// the signal is on. Only requests for web addresses get it.
pub(crate) fn with_signal(
    request: Option<&Request>,
    handler: Option<ResourceRequestHandler>,
) -> Option<ResourceRequestHandler> {
    // Off, as at first: nothing to look at.
    if !on() {
        return handler;
    }
    let web = request.is_some_and(|r| {
        let url = CefString::from(&r.url()).to_string();
        url.starts_with("https://") || url.starts_with("http://")
    });
    if web {
        Some(Signal::new(handler))
    } else {
        handler
    }
}

// Adds the header, then does what `then` would have done with the request.
wrap_resource_request_handler! {
    struct Signal {
        then: Option<ResourceRequestHandler>,
    }
    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let mut request = request;
            if let Some(request) = request.as_deref_mut() {
                request.set_header_by_name(Some(&CefString::from("Sec-GPC")), Some(&CefString::from("1")), 1);
            }
            match &self.then {
                Some(then) => then.on_before_resource_load(browser, frame, request, callback),
                None => ReturnValue::CONTINUE,
            }
        }
    }
}
