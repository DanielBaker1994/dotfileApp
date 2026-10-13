//! A small HTML pane for app-built pages (the Jira ticket page and board
//! kanban): a [`super::quiet_web`] view, one `window.webkit.messageHandlers.
//! <channel>` string channel back to Rust, link clicks opened in the browser
//! (the page itself never navigates away), and page zoom (`PageZoomable`).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSNumber, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType, WKScriptMessage,
    WKScriptMessageHandler, WKUserContentController, WKWebView, WKWebViewConfiguration,
};
use std::cell::RefCell;

pub struct HandlerIvars {
    on_message: RefCell<Option<Box<dyn Fn(String)>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSWebPaneHandler"]
    #[ivars = HandlerIvars]
    pub struct WebPaneHandler;

    impl WebPaneHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _c: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            let any: &AnyObject = &body;
            if let Some(s) = any.downcast_ref::<NSString>() {
                if let Some(cb) = self.ivars().on_message.borrow().as_ref() {
                    cb(s.to_string());
                }
            }
        }
    }

    unsafe impl NSObjectProtocol for WebPaneHandler {}
    unsafe impl WKScriptMessageHandler for WebPaneHandler {}

    unsafe impl WKNavigationDelegate for WebPaneHandler {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // `loadHTMLString` is `.other`; a clicked link opens outside.
            if unsafe { action.navigationType() } == WKNavigationType::Other {
                handler.call((WKNavigationActionPolicy::Allow,));
                return;
            }
            let req = unsafe { action.request() };
            if let Some(url) = req.URL().and_then(|u| u.absoluteString()) {
                crate::platform::open_url(&url.to_string());
            }
            handler.call((WKNavigationActionPolicy::Cancel,));
        }
    }
);

/// The pane: its web view (add it to a superview) and the handler that
/// keeps the channel alive.
pub struct WebPane {
    pub web: Retained<WKWebView>,
    pub handler: Retained<WebPaneHandler>,
}

impl WebPane {
    pub fn new(mtm: MainThreadMarker, channel: &str, on_message: Box<dyn Fn(String)>) -> WebPane {
        let handler = WebPaneHandler::alloc(mtm).set_ivars(HandlerIvars { on_message: RefCell::new(Some(on_message)) });
        let handler: Retained<WebPaneHandler> = unsafe { msg_send![super(handler), init] };
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let ucc = unsafe { config.userContentController() };
        unsafe { ucc.addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), &NSString::from_str(channel)) };
        let web = super::quiet_web::new(mtm, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 240.0)), &config);
        let no = NSNumber::numberWithBool(false);
        let _: () = unsafe { msg_send![&*web, setValue: &*no, forKey: &*NSString::from_str("drawsBackground")] };
        unsafe { web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*handler))) };
        WebPane { web, handler }
    }

    pub fn load_html(&self, html: &str) {
        unsafe {
            self.web.loadHTMLString_baseURL(&NSString::from_str(html), None);
        }
    }

    pub fn eval(&self, js: &str) {
        unsafe { self.web.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
    }

    pub fn is_loading(&self) -> bool {
        unsafe { self.web.isLoading() }
    }

    pub fn page_zoom(&self) -> f64 {
        unsafe { self.web.pageZoom() }
    }

    pub fn set_page_zoom(&self, z: f64) {
        unsafe { self.web.setPageZoom(z) };
    }
}

/// A JS string literal for `s` (JSON-quoted).
pub fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}
