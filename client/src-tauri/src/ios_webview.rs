//! iOS host policy for the main `WKWebView`.
//!
//! Two jobs, both of them "stop the native host from fighting the renderer".
//!
//! ## 1. Safe area — the web viewport must be full-bleed
//!
//! `WKWebView` hosts its content in a `UIScrollView`. By default iOS lets that
//! scroll view adjust its own content inset for the safe area, which shrinks
//! the web *viewport*: `window.innerHeight` stops at the top of the home
//! indicator instead of the bottom of the screen, and `100dvh`/`height:100%`
//! resolve to that smaller box. The strip left over is not part of the DOM at
//! all — it is the native background showing through — so no amount of CSS can
//! paint over it. On screen it reads as a white band under the bottom tab bar.
//!
//! Measured on an iPhone 14 Pro Max screenshot (1290x2796 @3x): the band began
//! at y=2517 and was 279px tall = 93.0pt, which is exactly 59pt (top status
//! bar) + 34pt (home indicator) — proof that the viewport had been shrunk by
//! both insets, not just the bottom one.
//!
//! The renderer already owns safe-area handling: `index.html` sets
//! `viewport-fit=cover`, and `index.css` derives `--safe-bottom` from
//! `env(safe-area-inset-bottom)` to pad the tab bar. So the native side must
//! stop "helping": we turn the automatic inset adjustment off and let the CSS
//! contract be the single source of truth. `env(safe-area-inset-*)` keeps
//! reporting the real insets afterwards, so the existing padding still applies
//! — content stays clear of the notch and the home indicator while the
//! background reaches the screen edge.
//!
//! This is device-independent by construction: nothing here encodes a screen
//! size. Every iPhone/iPad generation reports its own insets, and the CSS
//! adapts. Non-notched devices report 0 and simply get a full-bleed viewport.
//!
//! ## 2. Status bar — it has to follow the app theme
//!
//! The status bar is drawn by iOS, outside the DOM, and its glyph colour comes
//! from the trait collection of the window's root view controller. The app
//! ships four themes (`dark`/`oled`/`light`/`peach`) selected in-app, *not*
//! from the system appearance, so iOS defaulted to dark glyphs — pure black
//! `(0,0,0)` on our `(30,30,31)` background, i.e. invisible.
//!
//! Setting `overrideUserInterfaceStyle` on the root view controller flips the
//! whole window's trait collection, which is what makes `.default` status-bar
//! style resolve to light glyphs. The renderer calls `ios_set_appearance` on
//! every theme change so this stays in sync instead of being hardcoded.

#[cfg(target_os = "ios")]
use tauri::Manager;

/// Applies the host policy to the main window.
///
/// Called once from `setup` and again on `RunEvent::Ready`; both passes are
/// idempotent (they only set absolute values, never accumulate).
pub fn configure_main(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "ios")]
    {
        apply_safe_area_policy(window)?;
        // The app boots dark (see `state/theme.ts`); the renderer corrects this
        // as soon as it reads the persisted theme.
        apply_appearance(window, true);
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = window;
    }
    Ok(())
}

/// Mirrors the renderer's theme onto the native chrome (status bar glyphs).
///
/// Registered as a Tauri command; the frontend calls it from the theme store.
/// A failure here is cosmetic only, so the renderer ignores errors.
#[tauri::command]
pub fn ios_set_appearance(app: tauri::AppHandle, dark: bool) {
    #[cfg(target_os = "ios")]
    {
        if let Some(win) = app.get_webview_window("main") {
            apply_appearance(&win, dark);
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, dark);
    }
}

/// `UIScrollViewContentInsetAdjustmentBehavior::Never`.
///
/// Spelled as the raw `NSInteger` discriminant so the crate doesn't need
/// `objc2-ui-kit` (and its per-type feature flags) just for one enum.
#[cfg(target_os = "ios")]
const CONTENT_INSET_ADJUSTMENT_NEVER: isize = 2;

/// `UIUserInterfaceStyle` discriminants.
#[cfg(target_os = "ios")]
const UI_USER_INTERFACE_STYLE_LIGHT: isize = 1;
#[cfg(target_os = "ios")]
const UI_USER_INTERFACE_STYLE_DARK: isize = 2;

#[cfg(target_os = "ios")]
fn apply_safe_area_policy(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    window.with_webview(|webview| {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;

        // SAFETY: `inner()` is the live `WKWebView` owned by this window, and
        // the closure runs on the main thread while the window is alive.
        unsafe {
            let ptr = webview.inner();
            if ptr.is_null() {
                // Not fatal: the app still runs, it just keeps the inset band.
                eprintln!("[tauri] ios: WKWebView pointer was null; safe-area policy skipped");
                return;
            }
            let wkwebview = &*ptr.cast::<AnyObject>();

            let scroll_view: Option<Retained<AnyObject>> =
                objc2::msg_send![wkwebview, scrollView];
            match scroll_view {
                Some(sv) => {
                    let _: () = objc2::msg_send![
                        &*sv,
                        setContentInsetAdjustmentBehavior: CONTENT_INSET_ADJUSTMENT_NEVER
                    ];
                }
                None => {
                    eprintln!("[tauri] ios: web view had no scroll view; safe-area policy skipped");
                }
            }
        }
    })
}

#[cfg(target_os = "ios")]
fn apply_appearance(window: &tauri::WebviewWindow, dark: bool) {
    let _ = window.with_webview(move |webview| {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;

        // SAFETY: as above — main thread, live objects, and every selector used
        // here exists since iOS 13 (our floor is 15.0).
        unsafe {
            let ptr = webview.inner();
            if ptr.is_null() {
                return;
            }
            let wkwebview = &*ptr.cast::<AnyObject>();

            // view -> window -> rootViewController. The controller owns the
            // status-bar appearance, so the override has to land there; setting
            // it on the web view alone would only restyle its own subtree.
            let win: Option<Retained<AnyObject>> = objc2::msg_send![wkwebview, window];
            let Some(win) = win else {
                return;
            };
            let vc: Option<Retained<AnyObject>> = objc2::msg_send![&*win, rootViewController];
            let Some(vc) = vc else {
                return;
            };

            let style = if dark {
                UI_USER_INTERFACE_STYLE_DARK
            } else {
                UI_USER_INTERFACE_STYLE_LIGHT
            };
            let _: () = objc2::msg_send![&*vc, setOverrideUserInterfaceStyle: style];
        }
    });
}
