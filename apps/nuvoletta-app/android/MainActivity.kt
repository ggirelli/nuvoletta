package dev.dioxus.main

import android.os.Bundle
import android.view.View
import android.view.WindowManager
import android.webkit.WebSettings
import android.webkit.WebView

typealias BuildConfig = dev.nuvoletta.app.BuildConfig

/**
 * Hardening that Rust cannot do without unsafe code (docs/ADR/0006-android-app.md):
 *
 * - FLAG_SECURE keeps note content out of screenshots, screen recordings and the recent-apps
 *   thumbnail. It is set before anything is shown and never cleared.
 * - The WebView is kept away from the platform autofill framework (which could offer to save the
 *   passphrase), and from its HTTP cache and form-data store. The page loads nothing from the
 *   network, so there is nothing to cache.
 */
class MainActivity : WryActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        super.onCreate(savedInstanceState)
    }

    override fun onWebViewCreate(webView: WebView) {
        webView.importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
        webView.settings.cacheMode = WebSettings.LOAD_NO_CACHE
        @Suppress("DEPRECATION")
        webView.settings.saveFormData = false
    }
}
