use crate::{
    context::ContextStack,
    element::{ElementKey, ElementType},
    hook::{AnyHook, Hook, Hooks},
    multimap::RemoveOnlyMultimap,
    props::{AnyProps, Props},
    render::{
        wake::{push_wake_enabled, ComponentWakeState},
        ComponentDrawer, ComponentUpdater, UpdateContext,
    },
};
use core::{
    any::{Any, TypeId},
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use futures::future::poll_fn;
use std::sync::Arc;
use taffy::NodeId;

pub(crate) struct ComponentHelper<C: Component> {
    _marker: PhantomData<C>,
}

impl<C: Component> ComponentHelper<C> {
    pub fn boxed() -> Box<dyn ComponentHelperExt> {
        Box::new(Self {
            _marker: PhantomData,
        })
    }
}

#[doc(hidden)]
pub trait ComponentHelperExt: Any + Send + Sync {
    fn new_component(&self, props: AnyProps) -> Box<dyn AnyComponent>;
    fn update_component(
        &self,
        component: &mut Box<dyn AnyComponent>,
        props: AnyProps,
        hooks: Hooks,
        updater: &mut ComponentUpdater,
    );
    fn component_type_id(&self) -> TypeId;
    fn component_type_name(&self) -> &'static str;
    fn copy(&self) -> Box<dyn ComponentHelperExt>;
}

impl<C: Component> ComponentHelperExt for ComponentHelper<C> {
    fn new_component(&self, props: AnyProps) -> Box<dyn AnyComponent> {
        Box::new(C::new(unsafe { props.downcast_ref_unchecked() }))
    }

    fn update_component(
        &self,
        component: &mut Box<dyn AnyComponent>,
        props: AnyProps,
        hooks: Hooks,
        updater: &mut ComponentUpdater,
    ) {
        component.update(props, hooks, updater);
    }

    fn component_type_id(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn component_type_name(&self) -> &'static str {
        core::any::type_name::<C>()
    }

    fn copy(&self) -> Box<dyn ComponentHelperExt> {
        Self::boxed()
    }
}

/// `Component` defines a component type and the methods required for instantiating and rendering
/// the component.
///
/// Most users will not need to implement this trait directly. This is only required for new, low
/// level component type definitions. Instead, the [`component`](macro@crate::component) macro should be used.
pub trait Component: Any + Send + Sync + Unpin {
    /// The type of properties that the component accepts.
    type Props<'a>: Props
    where
        Self: 'a;

    /// Creates a new instance of the component from a set of properties.
    fn new(props: &Self::Props<'_>) -> Self;

    /// Invoked whenever the properties of the component or layout may have changed.
    fn update(
        &mut self,
        _props: &mut Self::Props<'_>,
        _hooks: Hooks,
        _updater: &mut ComponentUpdater,
    ) {
    }

    /// Invoked to draw the component.
    fn draw(&mut self, _drawer: &mut ComponentDrawer<'_>) {}

    /// Invoked to determine whether a change has occurred that would require the component to be
    /// updated and redrawn.
    fn poll_change(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}

impl<C: Component> ElementType for C {
    type Props<'a> = C::Props<'a>;
}

#[doc(hidden)]
pub trait AnyComponent: Any + Send + Sync + Unpin {
    fn update(&mut self, props: AnyProps, hooks: Hooks, updater: &mut ComponentUpdater);
    fn draw(&mut self, drawer: &mut ComponentDrawer<'_>);
    fn poll_change(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()>;
}

impl<C: Any + Component> AnyComponent for C {
    fn update(&mut self, mut props: AnyProps, hooks: Hooks, updater: &mut ComponentUpdater) {
        Component::update(
            self,
            unsafe { props.downcast_mut_unchecked() },
            hooks,
            updater,
        );
    }

    fn draw(&mut self, drawer: &mut ComponentDrawer<'_>) {
        Component::draw(self, drawer);
    }

    fn poll_change(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Component::poll_change(self, cx)
    }
}

pub(crate) struct InstantiatedComponent {
    node_id: NodeId,
    component: Box<dyn AnyComponent>,
    children: Components,
    helper: Box<dyn ComponentHelperExt>,
    hooks: Vec<Box<dyn AnyHook>>,
    first_update: bool,
    has_transparent_layout: bool,
    skip_child_poll: bool,
    pending_change: bool,
    // Layout nodes exposed through transparent descendants during the last
    // update. Memo-like wrappers reuse this list when they retain children
    // without re-entering the skipped subtree.
    exposed_child_node_ids: Vec<NodeId>,
    // This update retained its children without re-entering them (memo
    // comparator matched, no retained child signaled a change) — the safety
    // invariant for the retained-blit draw fast path.
    subtree_retained: bool,
    // Absolute canvas rectangle this subtree drew into last frame. A blit is
    // only attempted when the current layout resolves to the same rectangle.
    cached_blit_bounds: Option<(usize, usize, usize, usize)>,
    // Push-mode wake routing (Phase W): the dirty bit any wake from this
    // component's hooks/futures lands on, plus the shared route to the render
    // loop. `proxy_waker` is the cached `Waker` built from it.
    wake_state: Arc<ComponentWakeState>,
    proxy_waker: Waker,
}

impl InstantiatedComponent {
    pub fn new(
        node_id: NodeId,
        props: AnyProps,
        helper: Box<dyn ComponentHelperExt>,
        wake_state: Arc<ComponentWakeState>,
    ) -> Self {
        let proxy_waker = Waker::from(Arc::clone(&wake_state));
        Self {
            node_id,
            component: helper.new_component(props),
            children: Components::default(),
            helper,
            hooks: Default::default(),
            first_update: true,
            has_transparent_layout: false,
            skip_child_poll: false,
            pending_change: false,
            exposed_child_node_ids: Vec::new(),
            subtree_retained: false,
            cached_blit_bounds: None,
            wake_state,
            proxy_waker,
        }
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub(crate) fn append_retained_layout_node_ids(&self, out: &mut Vec<NodeId>) {
        out.push(self.node_id);
        out.extend(self.exposed_child_node_ids.iter().copied());
    }

    pub fn component(&self) -> &dyn AnyComponent {
        &*self.component
    }

    pub fn update(
        &mut self,
        context: &mut UpdateContext<'_, '_>,
        unattached_child_node_ids: &mut Vec<NodeId>,
        component_context_stack: &mut ContextStack<'_>,
        props: AnyProps,
    ) {
        let exposed_start = unattached_child_node_ids.len();
        let mut updater = ComponentUpdater::new(
            self.node_id,
            &mut self.children,
            unattached_child_node_ids,
            context,
            component_context_stack,
            Arc::clone(&self.wake_state),
        );
        self.hooks.pre_component_update(&mut updater);
        self.helper.update_component(
            &mut self.component,
            props,
            Hooks::new(&mut self.hooks, self.first_update, Some(&self.proxy_waker)),
            &mut updater,
        );
        self.hooks.post_component_update(&mut updater);
        self.hooks.post_component_effects(&mut updater);
        self.first_update = false;
        self.has_transparent_layout = updater.has_transparent_layout();
        self.skip_child_poll = updater.should_skip_child_poll();
        self.subtree_retained = updater.did_retain_children();
        self.exposed_child_node_ids = unattached_child_node_ids[exposed_start..].to_vec();
        self.pending_change = false;
    }

    pub fn draw(&mut self, drawer: &mut ComponentDrawer<'_>) {
        if self.has_transparent_layout {
            // If the component has a transparent layout, provide the first child's layout to the
            // hooks and component.
            if let Some(child) = self.children.components.iter().next().as_ref() {
                let child_node_id = child.node_id;
                // Retained-blit fast path: a memo-retained single-child
                // subtree is cell-identical to the previous frame, so restore
                // its rectangle from the previous canvas and skip the whole
                // subtree draw — the CC ink clean-node blit
                // (render-node-to-output.ts:452-480). The rectangle is the
                // union of the child's retained layout nodes: transparent
                // wrappers (memo children are usually `#[component]` functions,
                // themselves transparent) have zero-sized nodes of their own,
                // and the real footprint lives on the exposed descendants —
                // all positioned relative to this drawer's current node.
                // Guards (gate, bounds, clipping, cursor) live in
                // `try_retained_blit`; the single-child restriction keeps the
                // union equal to the subtree's full footprint.
                let single_child = self.children.components.iter().nth(1).is_none();
                let subtree_rect = if self.subtree_retained || single_child {
                    let mut ids = Vec::new();
                    child.append_retained_layout_node_ids(&mut ids);
                    drawer.union_layout_rect(&ids)
                } else {
                    None
                };
                if self.subtree_retained && single_child {
                    if let Some(rect) = subtree_rect {
                        if self.cached_blit_bounds == Some(rect)
                            && drawer.try_retained_blit(rect)
                        {
                            return;
                        }
                    }
                }
                drawer.for_child_node_layout(child_node_id, |drawer| {
                    self.hooks.pre_component_draw(drawer);
                    self.component.draw(drawer);
                });
                self.cached_blit_bounds = if single_child { subtree_rect } else { None };
            } else {
                self.hooks.pre_component_draw(drawer);
                self.component.draw(drawer);
                self.cached_blit_bounds = None;
            }
        } else {
            self.hooks.pre_component_draw(drawer);
            self.component.draw(drawer);
            // Only transparent memo wrappers ride the blit fast path; other
            // components draw their own content each frame.
            self.cached_blit_bounds = None;
        }

        if !drawer.take_skip_children() {
            drawer.with_clip_rect_for_children(|drawer| {
                self.children.draw(drawer);
            });
        }

        // CC Ink applies noSelect ops after writes/blits but before its
        // post-render selection/search overlay pass. Replay deferred noSelect
        // metadata here so post-draw hooks observe the current final bitmap;
        // keep the ops queued so later sibling blits can be repaired again
        // before ancestor/root overlay hooks run.
        drawer.replay_deferred_no_select();

        if self.has_transparent_layout {
            if let Some(child) = self.children.components.iter().next().as_ref() {
                drawer.for_child_node_layout(child.node_id, |drawer| {
                    self.hooks.post_component_draw(drawer);
                });
            } else {
                self.hooks.post_component_draw(drawer);
            }
        } else {
            self.hooks.post_component_draw(drawer);
        }
    }

    /// Render-phase settle pass (push mode only): absorbs state writes that
    /// the update phase itself produced, so the frame can re-run its update
    /// and commit the settled values instead of scheduling an empty follow-up
    /// frame. React analogue: setState during render re-renders before commit.
    ///
    /// Only hooks overriding [`Hook::settle_render_phase_change`] (synchronous
    /// state cells) respond; futures, terminal events and external stores are
    /// NOT polled here — those are outside changes and belong to the next
    /// frame. The dirty bit is peeked, never consumed, so a future woken on
    /// the same component is still harvested next frame. A settled change
    /// marks `pending_change` so memo wrappers re-enter the subtree on the
    /// re-run, exactly as a harvested change would.
    pub(crate) fn settle_render_phase(&mut self) -> bool {
        debug_assert!(push_wake_enabled());
        // Same subtree gate as the harvest scan, peeked rather than consumed.
        if !self.wake_state.is_subtree_dirty() {
            return false;
        }
        let mut settled = false;
        if self.wake_state.is_dirty()
            && self.hooks.settle_render_phase_change(&self.proxy_waker)
        {
            self.pending_change = true;
            settled = true;
        }
        for child in self.children.components.iter_mut() {
            if child.settle_render_phase() {
                self.pending_change = true;
                settled = true;
            }
        }
        settled
    }

    pub async fn wait(&mut self) {
        let mut self_mut = Pin::new(self);
        poll_fn(|cx| {
            // Push mode routes every component wake through the shared root
            // slot; keep it pointed at the loop's current waker. AtomicWaker
            // handles the register/wake race internally.
            if push_wake_enabled() {
                self_mut.wake_state.root().waker.register(cx.waker());
            }
            self_mut.as_mut().poll_change(cx)
        })
        .await;
    }

    fn poll_change(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let harvest = push_wake_enabled();
        // Subtree gate (dirty-path W2): no wake has landed on this component
        // or below since the last scan consumed the bit, so nothing here can
        // have changed and the whole subtree is skipped. Without it the scan
        // still visited every component just to read its dirty bit.
        if harvest && !self.wake_state.take_subtree_dirty() {
            return Poll::Pending;
        }
        #[cfg(test)]
        SCAN_DESCENTS.with(|count| count.set(count.get() + 1));
        // Harvest scan: hooks obey the Future contract (Pending ⇒ they re-poll
        // only after a wake), and in push mode every wake lands on this
        // component's dirty bit. A clean component's hook wakers are still
        // armed from the last poll, so skipping it cannot lose a change.
        // New components start dirty, so the first poll always arms.
        let self_dirty = if harvest {
            self.wake_state.take_dirty()
        } else {
            true
        };
        let proxy_waker = self.proxy_waker.clone();
        let mut proxy_cx = Context::from_waker(&proxy_waker);
        let component_status = if self_dirty {
            if harvest {
                Pin::new(&mut *self.component).poll_change(&mut proxy_cx)
            } else {
                Pin::new(&mut *self.component).poll_change(cx)
            }
        } else {
            Poll::Pending
        };
        let children_status = if self.skip_child_poll {
            Poll::Pending
        } else {
            Pin::new(&mut self.children).poll_change(cx)
        };
        let hooks_status = if self_dirty {
            if harvest {
                Pin::new(&mut self.hooks).poll_change(&mut proxy_cx)
            } else {
                Pin::new(&mut self.hooks).poll_change(cx)
            }
        } else {
            Poll::Pending
        };
        if component_status.is_ready() || children_status.is_ready() || hooks_status.is_ready() {
            self.pending_change = true;
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

// Components the harvest scan descended into (past the subtree gate), for
// tests that prove a clean subtree is not walked. Thread-local: the mock
// render loop runs on the test's own thread, so parallel tests do not
// pollute each other's count.
#[cfg(test)]
thread_local! {
    pub(crate) static SCAN_DESCENTS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[derive(Default)]
pub(crate) struct Components {
    pub components: RemoveOnlyMultimap<ElementKey, InstantiatedComponent>,
}

impl Components {
    pub fn has_pending_change(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.pending_change)
    }

    pub fn draw(&mut self, drawer: &mut ComponentDrawer<'_>) {
        for component in self.components.iter_mut() {
            if component.has_transparent_layout {
                component.draw(drawer);
            } else {
                drawer.for_child_node_layout(component.node_id, |drawer| {
                    component.draw(drawer);
                });
            }
        }
    }

    pub fn poll_change(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut is_ready = false;
        for component in self.components.iter_mut() {
            if Pin::new(&mut *component).poll_change(cx).is_ready() {
                is_ready = true;
            }
        }
        if is_ready {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
