use jni::{
    objects::{GlobalRef, JClass, JObject},
    JNIEnv,
};

use super::jni_context::JniContext;
use crate::Result;

pub(crate) struct Notifier {
    notifier: GlobalRef,
}

type NotifierCallback = dyn Fn(&mut JNIEnv, &JObject);

#[repr(C)]
struct NativeNotifierState {
    on_notify: unsafe extern "C" fn(
        state: *mut NativeNotifierState,
        env: *mut jni::sys::JNIEnv,
        argument: jni::sys::jobject,
    ),
    on_destroy: unsafe extern "C" fn(state: *mut NativeNotifierState),
}

#[repr(C)]
struct NativeNotifierCallbackState {
    base: NativeNotifierState,
    callback: Box<NotifierCallback>,
}

unsafe extern "C" fn native_notifier_on_notify(
    state: *mut NativeNotifierState,
    env: *mut jni::sys::JNIEnv,
    argument: jni::sys::jobject,
) {
    if state.is_null() || env.is_null() {
        return;
    }

    let state = unsafe { &*(state as *mut NativeNotifierCallbackState) };
    let mut env = match unsafe { JNIEnv::from_raw(env) } {
        Ok(env) => env,
        Err(_) => return,
    };
    let argument = unsafe { JObject::from_raw(argument) };
    (state.callback)(&mut env, &argument);
}

unsafe extern "C" fn native_notifier_on_destroy(state: *mut NativeNotifierState) {
    if state.is_null() {
        return;
    }

    let _state: Box<NativeNotifierCallbackState> =
        unsafe { Box::from_raw(state as *mut NativeNotifierCallbackState) };
}

impl Notifier {
    pub fn new<F>(callback: F) -> Result<Self>
    where
        F: Fn(&mut JNIEnv, &JObject) + 'static,
    {
        let callback: Box<NotifierCallback> = Box::new(callback);
        let state = NativeNotifierCallbackState {
            base: NativeNotifierState {
                on_notify: native_notifier_on_notify,
                on_destroy: native_notifier_on_destroy,
            },
            callback,
        };

        let context = JniContext::get()?;
        let mut env = context.java_vm().get_env()?;
        let class_loader = context.class_loader();
        let notifier_class: JClass = env
            .call_method(
                class_loader.as_obj(),
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[(&env.new_string("dev/irondash/engine_context/NativeNotifier")?).into()],
            )?
            .l()?
            .into();
        let callback_addr = Box::into_raw(Box::new(state)) as *mut NativeNotifierState as i64;
        let instance = env.new_object(notifier_class, "(J)V", &[callback_addr.into()])?;
        let instance = env.new_global_ref(instance)?;
        Ok(Self { notifier: instance })
    }

    fn get_native_data(env: &mut JNIEnv, obj: &JObject) -> Result<i64> {
        Ok(env.get_field(obj, "mNativeData", "J")?.j()?)
    }

    fn set_native_data(env: &mut JNIEnv, obj: &JObject, data: i64) -> Result<()> {
        env.set_field(obj, "mNativeData", "J", data.into())?;
        Ok(())
    }

    pub fn as_obj(&self) -> &JObject {
        self.notifier.as_obj()
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        let env = JniContext::get()
            .ok()
            .map(|c| c.java_vm())
            .and_then(|e| e.get_env().ok());
        if let Some(mut env) = env {
            env.call_method(self.notifier.as_obj(), "destroy", "()V", &[])
                .ok();
        }
    }
}

#[no_mangle]
extern "system" fn Java_dev_irondash_engine_1context_NativeNotifier_onNotify(
    mut env: JNIEnv,
    obj: JObject,
    argument: JObject,
) {
    let state = Notifier::get_native_data(&mut env, &obj)
        .ok()
        .map(|data| data as *mut NativeNotifierState)
        .unwrap_or(std::ptr::null_mut());
    if !state.is_null() {
        unsafe {
            ((*state).on_notify)(state, env.get_native_interface(), argument.as_raw());
        }
    }
}

#[no_mangle]
extern "system" fn Java_dev_irondash_engine_1context_NativeNotifier_destroy(
    mut env: JNIEnv,
    obj: JObject,
) {
    let state = Notifier::get_native_data(&mut env, &obj)
        .ok()
        .map(|data| data as *mut NativeNotifierState)
        .unwrap_or(std::ptr::null_mut());
    if !state.is_null() {
        Notifier::set_native_data(&mut env, &obj, 0).ok();
        unsafe {
            ((*state).on_destroy)(state);
        }
    }
}
