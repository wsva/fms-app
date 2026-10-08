package com.wsva.fms_app

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    // Keep the WebView above the soft keyboard. `enableEdgeToEdge()` makes the
    // window draw behind the system bars and, critically, stops it resizing for
    // the IME — so the keyboard overlaps a bottom-anchored input (the Device
    // Chat composer, and any form field) instead of pushing it up. Shrink the
    // content view by the IME bottom inset so the layout viewport tracks the
    // visible area; when the keyboard is hidden the inset is 0 and CSS
    // `env(safe-area-inset-*)` still owns the nav-bar gutter.
    // NOTE: re-apply after `tauri android init` regenerates this file.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { v, insets ->
      val imeBottom = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom
      v.setPadding(v.paddingLeft, v.paddingTop, v.paddingRight, imeBottom)
      insets
    }
  }
}
