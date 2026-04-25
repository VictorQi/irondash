package com.example.example

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import android.content.Intent

class MainActivity: FlutterActivity() {

	companion object {
		private const val HOST_CHANNEL = "com.example.example/host"
	}

	override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
		super.configureFlutterEngine(flutterEngine)

		MethodChannel(flutterEngine.dartExecutor.binaryMessenger, HOST_CHANNEL)
			.setMethodCallHandler { call: MethodCall, result: MethodChannel.Result ->
				when (call.method) {
					"launchMultiEngineHost" -> {
						startActivity(Intent(this, MultiEngineActivity::class.java))
						result.success(null)
					}
					else -> result.notImplemented()
				}
			}
	}

}
