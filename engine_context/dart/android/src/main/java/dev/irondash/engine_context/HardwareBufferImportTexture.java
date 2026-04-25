package dev.irondash.engine_context;

import android.graphics.ColorSpace;
import android.hardware.HardwareBuffer;
import android.media.Image;
import android.media.ImageReader;
import android.os.Build;
import android.view.Surface;

import androidx.annotation.Keep;
import androidx.annotation.NonNull;
import androidx.annotation.Nullable;

import io.flutter.view.TextureRegistry;
import io.flutter.view.TextureRegistry.ImageTextureEntry;

@Keep
@SuppressWarnings("unused")
public final class HardwareBufferImportTexture implements AutoCloseable {
    private static final int MAX_IMAGES = 4;

    private final ImageTextureEntry textureEntry;
    private boolean released = false;
    private int width = 1;
    private int height = 1;
    private long nativeHandle;
    @Nullable
    private ImageReader reader;

    public HardwareBufferImportTexture(@NonNull TextureRegistry registry, long nativeHandle) {
        if (!isSupported()) {
            throw new UnsupportedOperationException(unavailableReason());
        }
        this.nativeHandle = nativeHandle;
        this.textureEntry = registry.createImageTexture();
    }

    public static boolean isSupported() {
        return Build.VERSION.SDK_INT >= 29 && nativeIsSurfaceAttachAndQueueSupported();
    }

    @NonNull
    public static String unavailableReason() {
        if (Build.VERSION.SDK_INT < 29) {
            return "Android hardware-buffer consumer import requires API 29+";
        }
        if (!nativeIsSurfaceAttachAndQueueSupported()) {
            return "Surface.attachAndQueueBufferWithColorSpace(HardwareBuffer, ColorSpace) is unavailable on this Android runtime";
        }
        return "Android hardware-buffer consumer import is available";
    }

    public long id() {
        return textureEntry.id();
    }

    public void setSize(int width, int height) {
        ensureOpen();
        width = Math.max(1, width);
        height = Math.max(1, height);
        if (this.width == width && this.height == height) {
            return;
        }
        this.width = width;
        this.height = height;
        closeReader();
    }

    public void queueHardwareBuffer(@NonNull HardwareBuffer hardwareBuffer, long releaseToken) {
        ensureOpen();
        Image image = null;
        try {
            image = acquireImportedImage(hardwareBuffer);
            textureEntry.pushImage(image);
            image = null;
            if (!released && nativeHandle != 0) {
                nativeOnImageReleased(nativeHandle, releaseToken);
            }
        } finally {
            if (image != null) {
                image.close();
            }
            hardwareBuffer.close();
        }
    }

    @Override
    public void close() {
        if (released) {
            return;
        }
        released = true;
        closeReader();
        textureEntry.release();
        nativeHandle = 0;
    }

    @NonNull
    private Image acquireImportedImage(@NonNull HardwareBuffer hardwareBuffer) {
        if (!nativeIsSurfaceAttachAndQueueSupported()) {
            throw new UnsupportedOperationException(unavailableReason());
        }
        ensureReader();
        final Surface surface = reader.getSurface();
        nativeAttachAndQueueBufferToSurface(
                surface,
                hardwareBuffer,
                ColorSpace.get(ColorSpace.Named.SRGB).getId());
        final Image image = reader.acquireLatestImage();
        if (image != null) {
            return image;
        }
        throw new IllegalStateException(
                "Surface.attachAndQueueBufferWithColorSpace did not produce an Image");
    }

    private void ensureOpen() {
        if (released) {
            throw new IllegalStateException("HardwareBufferImportTexture is already closed");
        }
    }

    private void ensureReader() {
        if (reader != null) {
            return;
        }
        reader = ImageReader.newInstance(width, height, HardwareBuffer.RGBA_8888, MAX_IMAGES);
    }

    private void closeReader() {
        if (reader != null) {
            textureEntry.pushImage(null);
            reader.close();
            reader = null;
        }
    }

    private static native boolean nativeIsSurfaceAttachAndQueueSupported();

    private static native void nativeAttachAndQueueBufferToSurface(
            @NonNull Surface surface,
            @NonNull HardwareBuffer hardwareBuffer,
            int colorSpaceId);

    private static native void nativeOnImageReleased(long nativeHandle, long releaseToken);
}