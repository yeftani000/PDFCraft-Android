package ai.craft.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Bundle
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import android.view.KeyEvent
import android.webkit.MimeTypeMap
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import com.google.androidgamesdk.GameActivity
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * Hosts the Rust app (`libpdfcraft_android.so`, entry point `android_main`) and does the things
 * that need the Android SDK: links, the document picker, the clipboard, and keeping documents in
 * step with where they came from.
 *
 * Rust calls the public methods below over JNI from its own thread (see craft-android). Everything
 * here returns quickly: the picker result comes back later through [nativeOnPicked], because the
 * native thread must stay free to answer the activity lifecycle while the picker is on screen.
 *
 * Documents. A picked or "opened with" document is copied into the cache (the app edits that
 * copy). [sources] remembers where each copy came from; a background thread notices when the app
 * has saved a copy and writes it back to the original. If the original can't be written (a
 * read-only provider, or a one-off grant that has expired), the copy is exported to
 * Downloads/PdfCraft instead, replacing the previous export of that name.
 */
class MainActivity : GameActivity() {

  companion object {
    private const val TAG = "CraftAndroid"

    /** Keep in step with `craft_android::OPEN_KEY`. */
    private const val OPEN_KEY = 0x7fff0001

    /** The folder, inside Pictures/Movies/Music/Downloads, that exports go to. */
    private const val FOLDER = "PdfCraft"

    private const val SOURCES_PREFS = "document-sources"
    private const val SYNC_INTERVAL_MS = 2000L
    private const val MAX_PIXELS = 64_000_000L
    private const val FILE_PROVIDER_SUFFIX = ".clipfiles"

    private const val WRITE_FLAGS =
        Intent.FLAG_GRANT_READ_URI_PERMISSION or
            Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
            Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION

    init {
      System.loadLibrary("pdfcraft_android")
    }
  }

  /** A document copy in the cache and the original it came from. */
  private class Source(val uri: Uri, var syncedModified: Long, var syncedLength: Long) {
    var pendingModified = -1L
    var pendingLength = -1L
  }

  /** Cache path of a copy -> its original. */
  private val sources = ConcurrentHashMap<String, Source>()

  /** Which Rust-side request the picker on screen belongs to. */
  private var pendingKey = -1

  // Pickers that also ask for (persistable) write access, so a save can go back to the original.
  private class OpenDocumentWritable : ActivityResultContracts.OpenDocument() {
    override fun createIntent(context: Context, input: Array<String>): Intent =
        super.createIntent(context, input).addFlags(WRITE_FLAGS)
  }

  private class OpenMultipleWritable : ActivityResultContracts.OpenMultipleDocuments() {
    override fun createIntent(context: Context, input: Array<String>): Intent =
        super.createIntent(context, input).addFlags(WRITE_FLAGS)
  }

  private val pickOne =
      registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        finishPick(listOfNotNull(uri))
      }

  private val pickMany =
      registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        finishPick(uris)
      }

  private val pickOneWritable =
      registerForActivityResult(OpenDocumentWritable()) { uri -> finishPick(listOfNotNull(uri)) }

  private val pickManyWritable =
      registerForActivityResult(OpenMultipleWritable()) { uris -> finishPick(uris) }

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    restoreSources()
    startSyncThread()
    handleIntent(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    handleIntent(intent)
  }

  /**
   * "Open with" from the Files app or a share: copies the document into the cache and posts it
   * under [OPEN_KEY] (craft_android::OPEN_KEY), which the app collects with `take_picked`.
   */
  @Suppress("DEPRECATION")
  private fun handleIntent(intent: Intent?) {
    if (intent == null) return
    val uri: Uri? =
        when (intent.action) {
          Intent.ACTION_VIEW -> intent.data
          Intent.ACTION_SEND -> intent.getParcelableExtra(Intent.EXTRA_STREAM)
          else -> null
        }
    if (uri == null) return
    Thread { copyToCache(uri)?.let { nativeOnPicked(OPEN_KEY, arrayOf(it)) } }.start()
  }

  /** Implemented in Rust: `Java_ai_craft_android_MainActivity_nativeOnPicked`. */
  private external fun nativeOnPicked(key: Int, paths: Array<String>)

  // ---------------------------------------------------------------------------------------------
  // Typing
  // ---------------------------------------------------------------------------------------------

  /** Implemented in Rust (text.rs): a key went down; Rust decides whether it types a character. */
  private external fun nativeOnTypedKey(
      keyCode: Int,
      unicode: Int,
      unicodeNoAlt: Int,
      shift: Boolean,
      capsLock: Boolean,
      ctrlOrMeta: Boolean,
  )

  /** Implemented in Rust (text.rs): a multi-character key event. */
  private external fun nativeOnTypedText(text: String)

  /**
   * Reports typed characters to Rust, resolved with Android's own key maps. winit's translation
   * depends on the input device's key character map, and on some devices that answers only while
   * Alt is held; asking Android directly (and again without Alt) doesn't. The event is still passed
   * on so winit sees every key for shortcuts and navigation; Rust drops the duplicate text.
   */
  @Suppress("DEPRECATION")
  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    try {
      when (event.action) {
        KeyEvent.ACTION_DOWN -> {
          val alt = KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON or KeyEvent.META_ALT_RIGHT_ON
          nativeOnTypedKey(
              event.keyCode,
              event.unicodeChar,
              event.getUnicodeChar(event.metaState and alt.inv()),
              event.isShiftPressed,
              event.isCapsLockOn,
              event.isCtrlPressed || event.isMetaPressed,
          )
        }
        KeyEvent.ACTION_MULTIPLE -> event.characters?.takeIf { it.isNotEmpty() }?.let { nativeOnTypedText(it) }
      }
    } catch (e: Exception) {
      Log.w(TAG, "Couldn't report a key press", e)
    }
    return super.dispatchKeyEvent(event)
  }

  /** Opens a link or media URL in whichever app handles it. */
  fun openUrl(url: String): Boolean =
      try {
        startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        true
      } catch (e: Exception) {
        Log.w(TAG, "No app can open $url", e)
        false
      }

  // ---------------------------------------------------------------------------------------------
  // Document picker
  // ---------------------------------------------------------------------------------------------

  /**
   * Shows the system document picker for [mimeTypes]; the result goes to [nativeOnPicked]. Picks
   * that include PDFs ask for write access too, so that saving can overwrite the original.
   */
  fun requestPick(key: Int, mimeTypes: Array<String>, multiple: Boolean) {
    runOnUiThread {
      pendingKey = key
      val types = if (mimeTypes.isEmpty()) arrayOf("*/*") else mimeTypes
      val writable = types.contains("application/pdf")
      try {
        when {
          multiple && writable -> pickManyWritable.launch(types)
          multiple -> pickMany.launch(types)
          writable -> pickOneWritable.launch(types)
          else -> pickOne.launch(types)
        }
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't open the document picker", e)
        finishPick(emptyList())
      }
    }
  }

  /** Copies the chosen documents into the cache (off the UI thread) and hands the paths to Rust. */
  private fun finishPick(uris: List<Uri>) {
    val key = pendingKey
    pendingKey = -1
    if (key < 0) return
    Thread {
          val paths =
              uris
                  .mapNotNull { uri ->
                    keepAccess(uri)
                    copyToCache(uri)
                  }
                  .toTypedArray()
          nativeOnPicked(key, paths)
        }
        .start()
  }

  /** Keeps read and write access to a picked document across restarts, where the provider allows. */
  private fun keepAccess(uri: Uri) {
    try {
      contentResolver.takePersistableUriPermission(
          uri,
          Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
      )
    } catch (e: Exception) {
      try {
        contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
      } catch (e2: Exception) {
        Log.d(TAG, "No persistable access to $uri")
      }
    }
  }

  private fun copyToCache(uri: Uri): String? =
      try {
        val dir = File(cacheDir, "picks").apply { mkdirs() }
        // Keep the original name (and so its extension), which Rust uses to tell file types apart.
        val name = displayName(uri)?.replace('/', '_') ?: "file"
        val out = File(dir, "${System.nanoTime()}_$name")
        contentResolver.openInputStream(uri)?.use { input ->
          out.outputStream().use { input.copyTo(it) }
        } ?: throw IllegalStateException("couldn't open $uri")
        rememberSource(out, uri)
        out.absolutePath
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't read $uri", e)
        null
      }

  private fun displayName(uri: Uri): String? =
      try {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
          if (it.moveToFirst()) it.getString(0) else null
        }
      } catch (e: Exception) {
        null
      }

  // ---------------------------------------------------------------------------------------------
  // Keeping documents in step with their originals
  // ---------------------------------------------------------------------------------------------

  private fun rememberSource(copy: File, original: Uri) {
    sources[copy.absolutePath] = Source(original, copy.lastModified(), copy.length())
    val prefs = getSharedPreferences(SOURCES_PREFS, Context.MODE_PRIVATE)
    val edit = prefs.edit().putString(copy.absolutePath, original.toString())
    // Forget copies that Android has cleared out of the cache.
    for (path in prefs.all.keys) {
      if (!File(path).exists()) edit.remove(path)
    }
    edit.apply()
  }

  /** After a restart, copies still in the cache keep their link to the original. */
  private fun restoreSources() {
    val prefs = getSharedPreferences(SOURCES_PREFS, Context.MODE_PRIVATE)
    for ((path, value) in prefs.all) {
      val file = File(path)
      if (value is String && file.exists()) {
        sources[path] = Source(Uri.parse(value), file.lastModified(), file.length())
      }
    }
  }

  private fun startSyncThread() {
    Thread {
          while (true) {
            try {
              Thread.sleep(SYNC_INTERVAL_MS)
              syncOnce()
            } catch (e: InterruptedException) {
              return@Thread
            } catch (e: Exception) {
              Log.w(TAG, "Document sync failed", e)
            }
          }
        }
        .apply {
          isDaemon = true
          name = "document-sync"
        }
        .start()
  }

  /** Writes back every copy the app has saved since the last pass. */
  private fun syncOnce() {
    for ((path, source) in sources) {
      val file = File(path)
      if (!file.exists()) {
        sources.remove(path)
        continue
      }
      val modified = file.lastModified()
      val length = file.length()
      if (modified == source.syncedModified && length == source.syncedLength) {
        source.pendingModified = -1L
        continue
      }
      // Changed: wait until it has stayed the same for a whole pass (the app may still be writing).
      if (modified != source.pendingModified || length != source.pendingLength) {
        source.pendingModified = modified
        source.pendingLength = length
        continue
      }
      writeBack(file, source, modified, length)
    }
  }

  private fun writeBack(file: File, source: Source, modified: Long, length: Long) {
    val shown = originalName(file)
    val saved =
        try {
          // "wt" truncates; a few providers only accept "w".
          val out =
              try {
                contentResolver.openOutputStream(source.uri, "wt")
              } catch (e: Exception) {
                contentResolver.openOutputStream(source.uri, "w")
              }
          out?.use { stream -> file.inputStream().use { it.copyTo(stream) } } != null
        } catch (e: Exception) {
          Log.w(TAG, "Couldn't write back to ${source.uri}", e)
          false
        }
    if (saved) {
      toast("Saved $shown")
    } else if (publishToMediaStore(file, shown)) {
      toast("Couldn't change the original. Saved a copy to ${mediaTarget(mimeOf(file)).second}/$FOLDER")
    } else {
      toast("Couldn't save $shown")
    }
    source.syncedModified = modified
    source.syncedLength = length
    source.pendingModified = -1L
  }

  /** The cache copies are named "<nanos>_<original name>". */
  private fun originalName(file: File): String = file.name.replace(Regex("^\\d+_"), "")

  private fun toast(message: String) {
    runOnUiThread { Toast.makeText(this, message, Toast.LENGTH_SHORT).show() }
  }

  // ---------------------------------------------------------------------------------------------
  // Exports to the gallery / Downloads
  // ---------------------------------------------------------------------------------------------

  private fun mimeOf(file: File): String =
      MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase())
          ?: "application/octet-stream"

  /** The MediaStore collection and top-level folder for a MIME type. */
  private fun mediaTarget(mime: String): Pair<Uri, String> =
      when {
        mime.startsWith("image/") -> MediaStore.Images.Media.EXTERNAL_CONTENT_URI to Environment.DIRECTORY_PICTURES
        mime.startsWith("video/") -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI to Environment.DIRECTORY_MOVIES
        mime.startsWith("audio/") -> MediaStore.Audio.Media.EXTERNAL_CONTENT_URI to Environment.DIRECTORY_MUSIC
        else -> MediaStore.Downloads.EXTERNAL_CONTENT_URI to Environment.DIRECTORY_DOWNLOADS
      }

  /**
   * Copies a finished file the app saved into the downloads folder into Pictures, Movies, Music or
   * Downloads (in a PdfCraft folder), off the UI thread. The private copy stays: the app may keep
   * saving to it, and each later save replaces the exported file rather than adding another.
   */
  fun publishMedia(path: String) {
    Thread {
          val file = File(path)
          if (file.exists()) publishToMediaStore(file, originalName(file))
        }
        .start()
  }

  /** Writes [file] as [displayName] into the public folder, replacing an earlier export of that name. */
  private fun publishToMediaStore(file: File, displayName: String): Boolean =
      try {
        val mime = mimeOf(file)
        val (collection, folder) = mediaTarget(mime)
        val relativePath = "$folder/$FOLDER/"
        val existing =
            contentResolver
                .query(
                    collection,
                    arrayOf(MediaStore.MediaColumns._ID),
                    "${MediaStore.MediaColumns.DISPLAY_NAME}=? AND ${MediaStore.MediaColumns.RELATIVE_PATH}=?",
                    arrayOf(displayName, relativePath),
                    null,
                )
                ?.use { if (it.moveToFirst()) ContentUris.withAppendedId(collection, it.getLong(0)) else null }
        val replaced =
            existing != null &&
                try {
                  contentResolver.openOutputStream(existing, "wt")?.use { out ->
                    file.inputStream().use { it.copyTo(out) }
                  } != null
                } catch (e: Exception) {
                  false // not ours to overwrite: add a new one instead
                }
        if (!replaced) {
          val values =
              ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
                put(MediaStore.MediaColumns.MIME_TYPE, mime)
                put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath)
                put(MediaStore.MediaColumns.IS_PENDING, 1)
              }
          val target = contentResolver.insert(collection, values) ?: throw IllegalStateException("insert failed")
          contentResolver.openOutputStream(target)?.use { out -> file.inputStream().use { it.copyTo(out) } }
          values.clear()
          values.put(MediaStore.MediaColumns.IS_PENDING, 0)
          contentResolver.update(target, values, null, null)
        }
        true
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't publish ${file.name}", e)
        false
      }

  // ---------------------------------------------------------------------------------------------
  // Clipboard
  // ---------------------------------------------------------------------------------------------

  private val clipboard: ClipboardManager
    get() = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

  private fun currentClip(): ClipData? =
      try {
        clipboard.primaryClip?.takeIf { it.itemCount > 0 }
      } catch (e: Exception) {
        null // from Android 10 only the foreground app may read the clipboard
      }

  fun getClipboardText(): String? {
    val clip = currentClip() ?: return null
    // coerceToText turns a URI item into its address, which isn't wanted for an image or file.
    if (!clip.description.hasMimeType("text/*")) return null
    return try {
      clip.getItemAt(0).coerceToText(this)?.toString()
    } catch (e: Exception) {
      null
    }
  }

  fun getClipboardHtml(): String? {
    val clip = currentClip() ?: return null
    if (!clip.description.hasMimeType("text/html")) return null
    return clip.getItemAt(0).htmlText
  }

  fun setClipboardText(text: String): Boolean =
      try {
        clipboard.setPrimaryClip(ClipData.newPlainText("text", text))
        true
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't set the clipboard text", e)
        false
      }

  fun setClipboardHtml(html: String, alt: String): Boolean =
      try {
        clipboard.setPrimaryClip(ClipData.newHtmlText("html", alt, html))
        true
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't set the clipboard HTML", e)
        false
      }

  /** An image on the clipboard as `width, height (4 bytes each, big-endian) + RGBA`, or null. */
  fun getClipboardImage(): ByteArray? {
    val clip = currentClip() ?: return null
    val description = clip.description
    val isImage = (0 until description.mimeTypeCount).any { description.getMimeType(it).startsWith("image/") }
    val uri = if (isImage) clip.getItemAt(0).uri else null
    if (uri == null) return null
    return try {
      contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it) }?.let { encodeBitmap(it) }
    } catch (e: Exception) {
      Log.w(TAG, "Couldn't read the clipboard image", e)
      null
    }
  }

  private fun encodeBitmap(bitmap: Bitmap): ByteArray? {
    val width = bitmap.width
    val height = bitmap.height
    if (width.toLong() * height > MAX_PIXELS) return null
    val pixels = IntArray(width * height)
    bitmap.getPixels(pixels, 0, width, 0, 0, width, height)
    val out = ByteArray(8 + pixels.size * 4)
    for (i in 0 until 4) {
      out[i] = (width ushr (24 - 8 * i)).toByte()
      out[4 + i] = (height ushr (24 - 8 * i)).toByte()
    }
    var o = 8
    for (p in pixels) {
      out[o++] = (p ushr 16).toByte() // R
      out[o++] = (p ushr 8).toByte() // G
      out[o++] = p.toByte() // B
      out[o++] = (p ushr 24).toByte() // A
    }
    return out
  }

  /** Puts an image (straight RGBA, `width * height * 4` bytes) on the clipboard as a PNG. */
  fun setClipboardImage(width: Int, height: Int, rgba: ByteArray): Boolean =
      try {
        if (rgba.size != width * height * 4) throw IllegalArgumentException("size mismatch")
        val pixels = IntArray(width * height)
        for (i in pixels.indices) {
          val o = i * 4
          val r = rgba[o].toInt() and 0xFF
          val g = rgba[o + 1].toInt() and 0xFF
          val b = rgba[o + 2].toInt() and 0xFF
          val a = rgba[o + 3].toInt() and 0xFF
          pixels[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
        }
        val bitmap = Bitmap.createBitmap(pixels, width, height, Bitmap.Config.ARGB_8888)
        val dir = File(cacheDir, "clip").apply { mkdirs() }
        dir.listFiles()?.forEach { it.delete() } // only the newest image is on the clipboard
        val file = File(dir, "clip-${System.nanoTime()}.png")
        file.outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        val uri = FileProvider.getUriForFile(this, "$packageName$FILE_PROVIDER_SUFFIX", file)
        clipboard.setPrimaryClip(ClipData.newUri(contentResolver, "image", uri))
        true
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't set the clipboard image", e)
        false
      }

  /** Puts files from the app's own folders on the clipboard. */
  fun setClipboardFiles(paths: Array<String>): Boolean =
      try {
        val uris = paths.map { FileProvider.getUriForFile(this, "$packageName$FILE_PROVIDER_SUFFIX", File(it)) }
        if (uris.isEmpty()) {
          false
        } else {
          val clip = ClipData.newUri(contentResolver, "files", uris[0])
          uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
          clipboard.setPrimaryClip(clip)
          true
        }
      } catch (e: Exception) {
        Log.w(TAG, "Couldn't set the clipboard files", e)
        false
      }

  /** Files on the clipboard, copied into the cache. */
  fun getClipboardFiles(): Array<String>? {
    val clip = currentClip() ?: return null
    val paths =
        (0 until clip.itemCount).mapNotNull { index ->
          val uri = clip.getItemAt(index).uri
          if (uri != null) copyToCache(uri) else null
        }
    return if (paths.isEmpty()) null else paths.toTypedArray()
  }
}
