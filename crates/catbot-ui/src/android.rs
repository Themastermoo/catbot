//! The phone app's way into Android (stage 28.5): handing a downloaded update to Android's own
//! installer. The person confirms there; Android may first ask them to let Catbot install apps.
//!
//! The APK sits in the app's private files (`files/catbot/updates/downloads/`). Android's
//! installer reads it through the app's FileProvider (`<package>.updates`, set up in
//! scripts/build-android.sh), which shares that one folder for this one read.

use jni::JavaVM;
use jni::objects::{JObject, JValue};

pub fn install_apk(path: &str) -> Result<(), String> {
    let context = ndk_context::android_context();
    // SAFETY: the VM and activity pointers come from the running app and outlive this call.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(context.context().cast()) };
    let result = (|| -> jni::errors::Result<()> {
        let path = env.new_string(path)?;
        let file = env.new_object("java/io/File", "(Ljava/lang/String;)V", &[JValue::Object(&path)])?;
        let package = env.call_method(&activity, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
        let package = jni::objects::JString::from(package);
        let package: String = env.get_string(&package)?.into();
        let authority = env.new_string(format!("{package}.updates"))?;
        // The app's own classes (androidx) are found through its class loader, not the system's.
        let loader = env.call_method(&activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
        let name = env.new_string("androidx.core.content.FileProvider")?;
        let provider = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?;
        let uri = env
            .call_static_method(
                jni::objects::JClass::from(provider),
                "getUriForFile",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/io/File;)Landroid/net/Uri;",
                &[JValue::Object(&activity), JValue::Object(&authority), JValue::Object(&file)],
            )?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
        let kind = env.new_string("application/vnd.android.package-archive")?;
        env.call_method(&intent, "setDataAndType", "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&uri), JValue::Object(&kind)])?;
        // FLAG_GRANT_READ_URI_PERMISSION | FLAG_ACTIVITY_NEW_TASK
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(0x0000_0001 | 0x1000_0000)])?;
        env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        Ok(())
    })();
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    // `activity` is only borrowed: a JObject made from a raw pointer doesn't release it.
    result.map_err(|e| e.to_string())
}
