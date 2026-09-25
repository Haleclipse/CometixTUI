use crate::{ComponentDrawer, ComponentUpdater, ContextStack};
use core::{
    any::Any,
    pin::Pin,
    task::{Context, Poll, Waker},
};

/// A hook is a way to add behavior to a component. Hooks are called at various points in the
/// update and draw cycle.
///
/// Hooks are created by implementing this trait. All methods have default implementations, so
/// you only need to implement the ones you care about.
pub trait Hook: Unpin + Send {
    /// Called to determine if the hook has caused a change which requires its component to be
    /// redrawn.
    fn poll_change(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<()> {
        Poll::Pending
    }

    /// Called between render-phase update rounds (push-wake settle). Returns
    /// true when this hook absorbed a change produced *by the update phase
    /// itself* — a render-phase state write — so the update re-runs within the
    /// same frame instead of scheduling a follow-up frame.
    ///
    /// This is deliberately narrower than [`poll_change`](Self::poll_change):
    /// it must not drive futures, consume terminal events, or observe external
    /// stores. Those are outside changes and belong to the next frame, exactly
    /// as React re-renders only for setState called during render. Only
    /// synchronous state cells should override it. `waker` is the component's
    /// wake proxy, to re-arm with if the change is consumed.
    fn settle_render_phase_change(&mut self, _waker: &core::task::Waker) -> bool {
        false
    }

    /// Name shown by `IOCRAFT_DEBUG=settle` when this hook reports a
    /// render-phase change; state cells return their value type.
    fn settle_trace_name(&self) -> Option<&'static str> {
        None
    }

    /// Called before the component is updated.
    fn pre_component_update(&mut self, _updater: &mut ComponentUpdater) {}

    /// Called after the component is updated.
    fn post_component_update(&mut self, _updater: &mut ComponentUpdater) {}

    /// Called once every hook on the component has finished
    /// [`post_component_update`](Self::post_component_update) for this pass.
    ///
    /// Effects run in `post_component_update`, so this is the earliest point at
    /// which side effects they produced (such as queued output) can be
    /// delivered within the same update pass, before the component is drawn
    /// and before the render loop checks for exit.
    fn post_component_effects(&mut self, _updater: &mut ComponentUpdater) {}

    /// Called before the component is drawn.
    fn pre_component_draw(&mut self, _drawer: &mut ComponentDrawer) {}

    /// Called after the component is drawn.
    fn post_component_draw(&mut self, _drawer: &mut ComponentDrawer) {}
}

pub(crate) trait AnyHook: Hook {
    fn any_self_mut(&mut self) -> &mut dyn Any;
}

impl<T: Hook + 'static> AnyHook for T {
    fn any_self_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Hook for Vec<Box<dyn AnyHook>> {
    fn poll_change(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        let mut is_ready = false;
        let wake_trace = crate::render::wake::wake_trace_enabled();
        for (index, hook) in self.iter_mut().enumerate() {
            if let Poll::Ready(()) = Pin::new(&mut **hook).poll_change(cx) {
                is_ready = true;
                if wake_trace {
                    eprintln!(
                        "iocraft-wake-hook #{index} {}",
                        hook.settle_trace_name().unwrap_or("?")
                    );
                }
            }
        }

        if is_ready {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn settle_render_phase_change(&mut self, waker: &core::task::Waker) -> bool {
        let mut settled = false;
        for (index, hook) in self.iter_mut().enumerate() {
            if hook.settle_render_phase_change(waker) {
                settled = true;
                if crate::render::wake::settle_trace_enabled() {
                    // Hook slot index in call order plus the state's value
                    // type: enough to find the `use_state` that was written
                    // during render.
                    eprintln!(
                        "iocraft-settle-hook #{index} {}",
                        hook.settle_trace_name().unwrap_or("?")
                    );
                }
            }
        }
        settled
    }

    fn pre_component_update(&mut self, updater: &mut ComponentUpdater) {
        for hook in self.iter_mut() {
            hook.pre_component_update(updater);
        }
    }

    fn post_component_update(&mut self, updater: &mut ComponentUpdater) {
        for hook in self.iter_mut() {
            hook.post_component_update(updater);
        }
    }

    fn post_component_effects(&mut self, updater: &mut ComponentUpdater) {
        for hook in self.iter_mut() {
            hook.post_component_effects(updater);
        }
    }

    fn pre_component_draw(&mut self, drawer: &mut ComponentDrawer) {
        for hook in self.iter_mut() {
            hook.pre_component_draw(drawer);
        }
    }

    fn post_component_draw(&mut self, drawer: &mut ComponentDrawer) {
        for hook in self.iter_mut() {
            hook.post_component_draw(drawer);
        }
    }
}

/// A collection of hooks attached to a component.
///
/// Custom hooks can be defined by creating a trait with additional methods and implementing it for
/// `Hooks<'_, '_>`.
pub struct Hooks<'a, 'b: 'a> {
    hooks: &'a mut Vec<Box<dyn AnyHook>>,
    first_update: bool,
    hook_index: usize,
    pub(crate) context_stack: Option<&'a ContextStack<'b>>,
    // The owning component's wake proxy, so a hook reconfigured during render
    // can ask for one poll of this component's hooks. `None` only for hook
    // collections built outside a component update.
    waker: Option<&'a Waker>,
}

impl<'a> Hooks<'a, '_> {
    pub(crate) fn new(
        hooks: &'a mut Vec<Box<dyn AnyHook>>,
        first_update: bool,
        waker: Option<&'a Waker>,
    ) -> Self {
        Self {
            hooks,
            first_update,
            hook_index: 0,
            context_stack: None,
            waker,
        }
    }

    #[doc(hidden)]
    pub fn with_context_stack<'c, 'd>(
        &'c mut self,
        context_stack: &'c ContextStack<'d>,
    ) -> Hooks<'c, 'd> {
        Hooks {
            hooks: self.hooks,
            first_update: self.first_update,
            hook_index: self.hook_index,
            context_stack: Some(context_stack),
            waker: self.waker,
        }
    }

    /// Schedules one [`Hook::poll_change`] pass over this component's hooks
    /// after the current render.
    ///
    /// A hook re-arms the wakers it depends on inside `poll_change`, but its
    /// configuration is written during render. When a render changes what a
    /// hook waits for (an interval that was paused and is now running, a timer
    /// whose period changed), the hook must be polled once more before that
    /// new interest is registered — otherwise, under push-mode wake routing,
    /// nothing wakes the component and the hook never fires. This is the
    /// `Waker` contract applied to render-time reconfiguration: "poll me
    /// again". In pull mode every frame polls everything, so the call is a
    /// no-op there.
    pub fn request_poll(&self) {
        if let Some(waker) = self.waker {
            waker.wake_by_ref();
        }
    }

    /// If this is the component's first render, this function adds a new hook to the component and
    /// returns it.
    ///
    /// If it is a subsequent render, this function does nothing and returns the hook that was
    /// added during the first render.
    pub fn use_hook<H, F>(&mut self, f: F) -> &mut H
    where
        F: FnOnce() -> H,
        H: Hook + Unpin + 'static,
    {
        if self.first_update {
            self.hooks.push(Box::new(f()));
        }

        let idx = self.hook_index;
        self.hook_index += 1;
        self.hooks.get_mut(idx).and_then(|hook| hook.any_self_mut().downcast_mut::<H>()).expect("Unexpected hook type! Most likely you've violated the rules of hooks and called this hook in a different order than the previous render.")
    }
}
