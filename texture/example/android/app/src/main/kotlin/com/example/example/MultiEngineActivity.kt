package com.example.example

import android.content.Intent
import android.os.Bundle
import android.view.View
import android.widget.Button
import android.widget.TextView
import androidx.fragment.app.FragmentActivity
import io.flutter.FlutterInjector
import io.flutter.embedding.android.FlutterFragment
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.FlutterEngineCache
import io.flutter.embedding.engine.FlutterEngineGroup
import io.flutter.embedding.engine.dart.DartExecutor
import io.flutter.plugins.GeneratedPluginRegistrant

class MultiEngineActivity : FragmentActivity() {

    companion object {
        private const val ENGINE_A_ID = "smoke_multi_engine_a"
        private const val ENGINE_B_ID = "smoke_multi_engine_b"
        private const val SLOT_A_ROUTE = "/multi-engine/a"
        private const val SLOT_B_ROUTE = "/multi-engine/b"
        private const val FRAGMENT_A_TAG = "flutter_fragment_a"
        private const val FRAGMENT_B_TAG = "flutter_fragment_b"
    }

    private lateinit var engineGroup: FlutterEngineGroup
    private lateinit var destroyPanelAButton: Button
    private lateinit var destroyPanelBButton: Button
    private lateinit var panelAPlaceholder: TextView
    private lateinit var panelBPlaceholder: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_multi_engine)

        destroyPanelAButton = findViewById(R.id.destroy_panel_a_button)
        destroyPanelBButton = findViewById(R.id.destroy_panel_b_button)
        panelAPlaceholder = findViewById(R.id.panel_a_placeholder)
        panelBPlaceholder = findViewById(R.id.panel_b_placeholder)

        ensureFlutterInitialized()
        engineGroup = FlutterEngineGroup(applicationContext)
        ensureEngine(ENGINE_A_ID, SLOT_A_ROUTE)
        ensureEngine(ENGINE_B_ID, SLOT_B_ROUTE)

        destroyPanelAButton.setOnClickListener {
            destroyPanel(
                engineId = ENGINE_A_ID,
                fragmentTag = FRAGMENT_A_TAG,
                placeholder = panelAPlaceholder,
                button = destroyPanelAButton,
            )
        }
        destroyPanelBButton.setOnClickListener {
            destroyPanel(
                engineId = ENGINE_B_ID,
                fragmentTag = FRAGMENT_B_TAG,
                placeholder = panelBPlaceholder,
                button = destroyPanelBButton,
            )
        }

        if (savedInstanceState == null) {
            supportFragmentManager.beginTransaction()
                .replace(
                    R.id.flutter_panel_a,
                    FlutterFragment.withCachedEngine(ENGINE_A_ID)
                        .shouldAttachEngineToActivity(false)
                        .build<FlutterFragment>(),
                    FRAGMENT_A_TAG,
                )
                .replace(
                    R.id.flutter_panel_b,
                    FlutterFragment.withCachedEngine(ENGINE_B_ID)
                        .shouldAttachEngineToActivity(false)
                        .build<FlutterFragment>(),
                    FRAGMENT_B_TAG,
                )
                .commitNow()
        }
    }

    override fun onPostResume() {
        super.onPostResume()
        flutterFragments().forEach { it.onPostResume() }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        flutterFragments().forEach { it.onNewIntent(intent) }
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        flutterFragments().forEach { it.onUserLeaveHint() }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        flutterFragments().forEach {
            it.onRequestPermissionsResult(requestCode, permissions, grantResults)
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        val fragment = flutterFragments().lastOrNull()
        if (fragment != null) {
            fragment.onBackPressed()
        } else {
            super.onBackPressed()
        }
    }

    override fun onDestroy() {
        if (isFinishing) {
            destroyEngine(ENGINE_A_ID)
            destroyEngine(ENGINE_B_ID)
        }
        super.onDestroy()
    }

    private fun ensureFlutterInitialized() {
        val loader = FlutterInjector.instance().flutterLoader()
        loader.startInitialization(applicationContext)
        loader.ensureInitializationComplete(applicationContext, emptyArray())
    }

    private fun ensureEngine(engineId: String, initialRoute: String) {
        val cache = FlutterEngineCache.getInstance()
        if (cache.contains(engineId)) {
            return
        }

        val loader = FlutterInjector.instance().flutterLoader()
        val engine = engineGroup.createAndRunEngine(
            applicationContext,
            DartExecutor.DartEntrypoint(loader.findAppBundlePath(), "main"),
            initialRoute,
        )
        GeneratedPluginRegistrant.registerWith(engine)
        cache.put(engineId, engine)
    }

    private fun destroyEngine(engineId: String) {
        val cache = FlutterEngineCache.getInstance()
        val engine = cache.get(engineId)
        cache.remove(engineId)
        engine?.destroy()
    }

    private fun destroyPanel(
        engineId: String,
        fragmentTag: String,
        placeholder: TextView,
        button: Button,
    ) {
        val fragment = supportFragmentManager.findFragmentByTag(fragmentTag)
        if (fragment != null) {
            supportFragmentManager.beginTransaction()
                .remove(fragment)
                .commitNow()
        }

        placeholder.visibility = View.VISIBLE
        button.isEnabled = false
        placeholder.post {
            destroyEngine(engineId)
        }
    }

    private fun flutterFragments(): List<FlutterFragment> {
        return supportFragmentManager.fragments.filterIsInstance<FlutterFragment>()
    }
}