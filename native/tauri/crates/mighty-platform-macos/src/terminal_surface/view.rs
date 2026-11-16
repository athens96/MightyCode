//! The NSView Ghostty draws in and takes input from: the vendored `GhosttyTerminal`'s AppKit view (`AppTerminalView.swift` and its
//! `+Input`, `+NSTextInputClient`, `+Lifecycle`, `+Snapshot` extensions, `TerminalKeyEventHandler@AppKit.swift`,
//! `TerminalTextInputHandler@AppKit.swift`) and Mighty's `HostTerminalView.swift`, ported onto libghostty's C functions. Main thread
//! only. libghostty sets its own `IOSurfaceLayer` on the plain layer-backed view it is given (Step 0's answer).
//!
//! - Keys go through `interpretKeyEvents:`, so the input method composes (Korean included): the marked text is Ghostty's preedit, a
//!   commit is the key's text, and a key that only edits (Shift-Tab → `insertBacktab:`) still reaches Ghostty as the hardware key.
//!   ⌘ shortcuts Ghostty does not bind go to the menu; ⌘C/⌘V also match by key position when the input source gives no Latin letter.
//! - The mouse selects, clicks and scrolls in Ghostty (its scrollback); a press takes the keyboard.
//! - Copy writes the user's own selection straight to the clipboard (no OSC 52 confirmation); paste and select all are Ghostty's.

use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_void};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSCursor, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSResponder, NSTextInputClient,
    NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
};

use mighty_platform::TerminalEvent;

use super::ghostty::*;
use super::keys::{self, MarkedText};
use super::runtime::{self, Shared};

pub struct Ivars {
    shared: Arc<Shared>,
    marked: RefCell<MarkedText>,
    /// While a key is interpreted: the texts the input method committed for it (`startCollectingText`).
    collected: RefCell<Option<Vec<String>>>,
    /// The key was interpreted as an editing command (`doCommandBySelector:`).
    interpreted: Cell<bool>,
    /// The timestamp of the ⌘/⌃ key equivalent let through to the menu once (`keyEcho.lastPerformKeyEvent`).
    last_perform_key: Cell<Option<f64>>,
    /// A right press over a selection: its release shows the Copy menu instead (`pointer.pendingSelectionMenuPoint`).
    menu_pending: Cell<bool>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirement this class breaks; it does not implement Drop.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct TerminalView;

    impl TerminalView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            runtime::guarded(false, || {
                // SAFETY: NSResponder's own answer first.
                let taken: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
                self.focus_changed(true);
                taken
            })
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            runtime::guarded(false, || {
                // SAFETY: NSResponder's own answer first.
                let given: bool = unsafe { msg_send![super(self), resignFirstResponder] };
                self.focus_changed(false);
                given
            })
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            runtime::guarded((), || {
                // SAFETY: NSView's own resize first.
                let () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.sync_size();
            })
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            runtime::guarded((), || {
                // SAFETY: NSView's own handling first.
                let () = unsafe { msg_send![super(self), viewDidChangeBackingProperties] };
                self.sync_size();
            })
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            runtime::guarded((), || {
                // SAFETY: NSView's own handling first.
                let () = unsafe { msg_send![super(self), updateTrackingAreas] };
                for area in self.trackingAreas().iter() {
                    self.removeTrackingArea(&area);
                }
                self.add_tracking_area();
            })
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.handle_key_down(event);
            })
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            runtime::guarded((), || {
                let Some(surface) = self.surface() else { return };
                let input = build_key_input(event, GHOSTTY_ACTION_RELEASE, None);
                // SAFETY: the live surface on the main thread.
                unsafe { ghostty_surface_key(surface, input) };
            })
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            runtime::guarded((), || {
                let Some(surface) = self.surface() else { return };
                if self.ivars().marked.borrow().has_marked_text() {
                    return;
                }
                let Some(pressed) = keys::modifier_pressed(event.keyCode(), event.modifierFlags().0 as u64) else { return };
                let input = build_key_input(event, if pressed { GHOSTTY_ACTION_PRESS } else { GHOSTTY_ACTION_RELEASE }, None);
                // SAFETY: the live surface on the main thread.
                unsafe { ghostty_surface_key(surface, input) };
            })
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            runtime::guarded(false, || {
                self.handle_key_equivalent(event)
            })
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.take_keyboard();
                self.mouse_button(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_LEFT);
            })
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.mouse_button(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_LEFT);
            })
        }

        /// Over a selection, a right press opens the Copy menu at its release (Ghostty's AppKit view); else it is Ghostty's.
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.take_keyboard();
                self.mouse_position(event);
                if self.has_selection() {
                    self.ivars().menu_pending.set(true);
                    return;
                }
                self.mouse_button(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_RIGHT);
            })
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            runtime::guarded((), || {
                if self.ivars().menu_pending.replace(false) {
                    self.mouse_position(event);
                    self.show_copy_menu(event);
                    return;
                }
                self.mouse_button(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_RIGHT);
            })
        }

        /// The cursor Ghostty asked for (`ACTION_MOUSE_SHAPE`) over the whole view.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            runtime::guarded((), || {
                let shape = self.ivars().shared.mouse_shape.load(Ordering::SeqCst);
                self.addCursorRect_cursor(self.bounds(), &cursor_for(shape));
            })
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.take_keyboard();
                self.mouse_button(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_MIDDLE);
            })
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.mouse_button(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_MIDDLE);
            })
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.mouse_position(event);
            })
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.mouse_position(event);
            })
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.mouse_position(event);
            })
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            runtime::guarded((), || {
                self.mouse_position(event);
            })
        }

        /// Ghostty clears a link's hover only for a position outside the view; skipped during a drag.
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, event: &NSEvent) {
            runtime::guarded((), || {
                if NSEvent::pressedMouseButtons() != 0 {
                    return;
                }
                if let Some(surface) = self.surface() {
                    let mods = keys::ghostty_mods(event.modifierFlags().0 as u64);
                    // SAFETY: the live surface on the main thread.
                    unsafe { ghostty_surface_mouse_pos(surface, -1.0, -1.0, mods) };
                }
            })
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            runtime::guarded((), || {
                let Some(surface) = self.surface() else { return };
                let mods = keys::scroll_mods(event.hasPreciseScrollingDeltas(), event.momentumPhase().0 as u64);
                // SAFETY: the live surface on the main thread.
                unsafe { ghostty_surface_mouse_scroll(surface, event.scrollingDeltaX(), event.scrollingDeltaY(), mods) };
                runtime::owe();
            })
        }

        /// The user's own copy: the selection straight to the clipboard (Mighty's `HostTerminalView`), never through OSC 52's
        /// confirmation.
        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            runtime::guarded((), || {
                self.copy_selection();
            })
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            runtime::guarded((), || {
                self.binding("paste_from_clipboard");
            })
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            runtime::guarded((), || {
                self.binding("select_all");
            })
        }
    }

    unsafe impl NSTextInputClient for TerminalView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement: NSRange) {
            runtime::guarded((), || {
                // Only text that came with a key event (Ghostty's AppKit view): the input method replaying a commit on its own types nothing.
                if NSApplication::sharedApplication(self.mtm()).currentEvent().is_none() {
                    return;
                }
                let text = text_of(string);
                self.unmark();
                let mut collected = self.ivars().collected.borrow_mut();
                match collected.as_mut() {
                    Some(texts) => texts.push(text),
                    None => {
                        drop(collected);
                        self.surface_text(&text);
                    }
                }
            })
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            runtime::guarded((), || {
                // A ⌘/⌃ key equivalent the menu passed on: sent again, now as a key (`keyEcho`).
                if let Some(last) = self.ivars().last_perform_key.get() {
                    let app = NSApplication::sharedApplication(self.mtm());
                    if let Some(current) = app.currentEvent()
                        && current.timestamp() == last
                    {
                        app.sendEvent(&current);
                        return;
                    }
                }
                self.ivars().interpreted.set(true);
            })
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, _replacement: NSRange) {
            runtime::guarded((), || {
                self.ivars().marked.borrow_mut().set(&text_of(string), (selected.location, selected.length));
                if self.ivars().collected.borrow().is_none() {
                    self.sync_preedit(true);
                }
            })
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            runtime::guarded((), || {
                self.unmark();
            })
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            runtime::guarded(NSRange::new(keys::NOT_FOUND as NSUInteger, 0), || {
                let (location, length) = self.ivars().marked.borrow().selected_range();
                NSRange::new(location, length)
            })
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            runtime::guarded(NSRange::new(keys::NOT_FOUND as NSUInteger, 0), || {
                let (location, length) = self.ivars().marked.borrow().marked_range();
                NSRange::new(location, length)
            })
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            runtime::guarded(false, || {
                self.ivars().marked.borrow().has_marked_text()
            })
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(&self, range: NSRange, actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
            runtime::guarded(None, || {
                // A closure: the method's own body may not use `?` (the macro converts its return value).
                let substring = || {
                    let (text, (location, length)) = self.ivars().marked.borrow().substring((range.location, range.length))?;
                    if !actual.is_null() {
                        // SAFETY: AppKit's out pointer for the range.
                        unsafe { *actual = NSRange::new(location, length) };
                    }
                    Some(NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&text)))
                };
                substring()
            })
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSString>> {
            NSArray::new()
        }

        /// Where the IME's candidate window goes: Ghostty's cursor cell, whose y is already the cell's bottom edge.
        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, _range: NSRange, _actual: NSRangePointer) -> NSRect {
            runtime::guarded(NSRect::ZERO, || {
                let Some(surface) = self.surface() else { return NSRect::ZERO };
                let (mut x, mut y, mut width, mut height) = (0.0, 0.0, 0.0, 0.0);
                // SAFETY: the live surface on the main thread and four out values.
                unsafe { ghostty_surface_ime_point(surface, &mut x, &mut y, &mut width, &mut height) };
                let local = NSRect::new(NSPoint::new(x, self.bounds().size.height - y), NSSize::new(width, height));
                let Some(window) = self.window() else { return local };
                window.convertRectToScreen(self.convertRect_toView(local, None))
            })
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _point: NSPoint) -> NSUInteger {
            keys::NOT_FOUND as NSUInteger
        }
    }
);

/// The text of what the input method hands over (an `NSString` or an `NSAttributedString`).
fn text_of(string: &AnyObject) -> String {
    if let Some(attributed) = string.downcast_ref::<NSAttributedString>() {
        return attributed.string().to_string();
    }
    string.downcast_ref::<NSString>().map(NSString::to_string).unwrap_or_default()
}

fn flags(event: &NSEvent) -> u64 {
    event.modifierFlags().0 as u64
}

/// `NSEvent.buildKeyInput`: the key's code, its modifiers, the modifiers spent on its text (all but Control and Command, from the
/// translated event when there is one) and its codepoint with no modifier.
fn build_key_input(event: &NSEvent, action: i32, translation_flags: Option<u64>) -> ghostty_input_key_s {
    let unshifted = if matches!(event.r#type(), NSEventType::KeyDown | NSEventType::KeyUp) {
        event
            .charactersByApplyingModifiers(NSEventModifierFlags(0))
            .and_then(|text| text.to_string().chars().next())
            .map_or(0, |c| c as u32)
    } else {
        0
    };
    ghostty_input_key_s {
        action,
        mods: keys::ghostty_mods(flags(event)),
        consumed_mods: keys::consumed_mods(translation_flags.unwrap_or_else(|| flags(event))),
        keycode: u32::from(event.keyCode()),
        text: std::ptr::null(),
        unshifted_codepoint: unshifted,
        composing: false,
    }
}

/// `NSEvent.filteredCharacters`: the key's text, none for a function key; a control character Ctrl made is asked again without
/// Ctrl, so Ghostty maps the physical key.
fn filtered_characters(event: &NSEvent) -> Option<String> {
    let characters = event.characters().map(|c| c.to_string());
    let text = keys::filtered_text(characters.as_deref())?.to_string();
    if !keys::is_control_character(&text) {
        return Some(text);
    }
    let without_control = NSEventModifierFlags(event.modifierFlags().0 & !NSEventModifierFlags::Control.0);
    let again = event.charactersByApplyingModifiers(without_control).map(|c| c.to_string());
    keys::filtered_text(again.as_deref()).map(str::to_string)
}

/// `KeyboardLayout.id`: the current keyboard input source's id (Carbon's text input sources).
fn keyboard_layout() -> Option<String> {
    use core_foundation::base::TCFType;
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> *mut c_void;
        fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
        static kTISPropertyInputSourceID: *const c_void;
    }
    // SAFETY: Carbon's documented calls: the copied source released once, its property borrowed (get rule) before that.
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return None;
        }
        let property = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
        let id = (!property.is_null())
            .then(|| core_foundation::string::CFString::wrap_under_get_rule(property as core_foundation::string::CFStringRef).to_string());
        core_foundation::base::CFRelease(source as core_foundation::base::CFTypeRef);
        id
    }
}

impl TerminalView {
    /// A view for `shared`'s surface at `frame` (layer-backed, mouse-tracked).
    pub fn new(mtm: MainThreadMarker, frame: NSRect, shared: Arc<Shared>) -> Retained<TerminalView> {
        let this = TerminalView::alloc(mtm).set_ivars(Ivars {
            shared,
            marked: RefCell::new(MarkedText::default()),
            collected: RefCell::new(None),
            interpreted: Cell::new(false),
            last_perform_key: Cell::new(None),
            menu_pending: Cell::new(false),
        });
        // SAFETY: NSView's designated initializer.
        let view: Retained<TerminalView> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);
        view.add_tracking_area();
        view.ivars().shared.view.store(Retained::as_ptr(&view) as *mut c_void, Ordering::SeqCst);
        view
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.ivars().shared
    }

    fn surface(&self) -> Option<ghostty_surface_t> {
        self.ivars().shared.surface()
    }

    fn add_tracking_area(&self) {
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::InVisibleRect
            | NSTrackingAreaOptions::ActiveAlways;
        // SAFETY: a tracking area owned by this view, with no user info.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), self.bounds(), options, Some(self), None)
        };
        self.addTrackingArea(&area);
    }

    /// The surface's pixel size and scale follow the view's (`TerminalSurfaceCoordinator.performMetricsSync`).
    pub fn sync_size(&self) {
        let Some(surface) = self.surface() else { return };
        let scale = self.window().map_or(1.0, |window| window.backingScaleFactor());
        let size = self.bounds().size;
        let (width, height) = ((size.width * scale).floor(), (size.height * scale).floor());
        if width < 1.0 || height < 1.0 {
            return;
        }
        // SAFETY: the live surface on the main thread.
        unsafe {
            ghostty_surface_set_content_scale(surface, scale, scale);
            ghostty_surface_set_size(surface, width as u32, height as u32);
        }
        runtime::owe();
    }

    fn focus_changed(&self, focused: bool) {
        if let Some(surface) = self.surface() {
            // SAFETY: the live surface on the main thread.
            unsafe { ghostty_surface_set_focus(surface, focused) };
        }
        self.ivars().shared.send(TerminalEvent::Focused(focused));
        runtime::owe();
    }

    /// Makes this view the window's first responder (a press in it, the page's `focus`).
    pub fn take_keyboard(&self) -> bool {
        let Some(window) = self.window() else { return false };
        let responder: &NSResponder = self;
        window.firstResponder().is_some_and(|current| std::ptr::eq(&*current, responder)) || window.makeFirstResponder(Some(self))
    }

    /// Whether this view has the keyboard now.
    pub fn has_keyboard(&self) -> bool {
        let responder: &NSResponder = self;
        self.window().and_then(|window| window.firstResponder()).is_some_and(|current| std::ptr::eq(&*current, responder))
    }

    fn mouse_point(&self, event: &NSEvent) -> (f64, f64) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        (point.x, self.bounds().size.height - point.y)
    }

    fn mouse_position(&self, event: &NSEvent) {
        let Some(surface) = self.surface() else { return };
        let (x, y) = self.mouse_point(event);
        // SAFETY: the live surface on the main thread.
        unsafe { ghostty_surface_mouse_pos(surface, x, y, keys::ghostty_mods(flags(event))) };
        runtime::owe();
    }

    fn mouse_button(&self, event: &NSEvent, state: i32, button: i32) {
        let Some(surface) = self.surface() else { return };
        self.mouse_position(event);
        // SAFETY: the live surface on the main thread.
        unsafe { ghostty_surface_mouse_button(surface, state, button, keys::ghostty_mods(flags(event))) };
        runtime::owe();
    }

    fn binding(&self, action: &str) -> bool {
        let Some(surface) = self.surface() else { return false };
        // SAFETY: the live surface on the main thread; the action's text for the call.
        let done = unsafe { ghostty_surface_binding_action(surface, action.as_ptr().cast(), action.len()) };
        runtime::owe();
        done
    }

    /// The selection's text, when there is one.
    fn selection(&self) -> Option<String> {
        let surface = self.surface()?;
        // SAFETY: the live surface on the main thread; the text is freed by libghostty.
        unsafe {
            if !ghostty_surface_has_selection(surface) {
                return None;
            }
            let mut text: ghostty_text_s = std::mem::zeroed();
            if !ghostty_surface_read_selection(surface, &mut text) {
                return None;
            }
            let selected = (!text.text.is_null() && text.text_len > 0)
                .then(|| String::from_utf8_lossy(std::slice::from_raw_parts(text.text.cast::<u8>(), text.text_len)).into_owned());
            ghostty_surface_free_text(surface, &mut text);
            selected
        }
    }

    fn has_selection(&self) -> bool {
        // SAFETY: the live surface on the main thread.
        self.surface().is_some_and(|surface| unsafe { ghostty_surface_has_selection(surface) })
    }

    /// The selection's Copy menu at the pointer (`selectionContextMenu`).
    fn show_copy_menu(&self, event: &NSEvent) {
        let main = self.mtm();
        let menu = NSMenu::new(main);
        // SAFETY: a menu item whose action is this view's `copy:`, targeted at it.
        let item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(main),
                &NSString::from_str("Copy"),
                Some(sel!(copy:)),
                &NSString::from_str(""),
            );
            item.setTarget(Some(self));
            item
        };
        menu.addItem(&item);
        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, self);
    }

    fn copy_selection(&self) -> bool {
        match self.selection() {
            Some(text) if !text.is_empty() => runtime::write_clipboard_text(&text),
            _ => false,
        }
    }

    /// Ghostty's text input (a paste: a program with bracketed paste gets it framed).
    pub fn surface_text(&self, text: &str) {
        let Some(surface) = self.surface() else { return };
        // SAFETY: the live surface on the main thread; the bytes for the call.
        unsafe { ghostty_surface_text(surface, text.as_ptr().cast(), text.len()) };
        runtime::owe();
    }

    /// A key pressed and released as if typed (`TerminalKeyPress`); an open composition is committed first, as ahead of a hardware
    /// key (`AppTerminalView.sendKey`).
    pub fn press(&self, key_code: u16, mods: i32, text: &str, unshifted: u32) {
        let Some(surface) = self.surface() else { return };
        self.commit_marked();
        let text = CString::new(text).unwrap_or_default();
        for action in [GHOSTTY_ACTION_PRESS, GHOSTTY_ACTION_RELEASE] {
            let input = ghostty_input_key_s {
                action,
                mods,
                consumed_mods: 0,
                keycode: u32::from(key_code),
                text: if action == GHOSTTY_ACTION_PRESS { text.as_ptr() } else { std::ptr::null() },
                unshifted_codepoint: unshifted,
                composing: false,
            };
            // SAFETY: the live surface on the main thread; the text lives for the call.
            unsafe { ghostty_surface_key(surface, input) };
        }
        runtime::owe();
    }

    /// `commitMarkedText`: an open composition typed as text on the key path (a keycode outside AppKit's table, so Ghostty encodes
    /// from the text alone), and the input method told to forget its copy.
    fn commit_marked(&self) {
        let Some(text) = self.ivars().marked.borrow().text().map(str::to_string) else { return };
        self.ivars().marked.borrow_mut().clear();
        self.sync_preedit(true);
        if let Some(surface) = self.surface() {
            let c_text = CString::new(text.as_str()).unwrap_or_default();
            let input = ghostty_input_key_s {
                action: GHOSTTY_ACTION_PRESS,
                mods: 0,
                consumed_mods: 0,
                keycode: 0xFFFF,
                text: c_text.as_ptr(),
                unshifted_codepoint: text.chars().next().map_or(0, |c| c as u32),
                composing: false,
            };
            // SAFETY: the live surface on the main thread; the text lives for the call.
            unsafe { ghostty_surface_key(surface, input) };
        }
        if let Some(context) = self.inputContext() {
            context.discardMarkedText();
        }
    }

    fn unmark(&self) {
        if !self.ivars().marked.borrow().has_marked_text() {
            return;
        }
        self.ivars().marked.borrow_mut().clear();
        self.sync_preedit(true);
    }

    /// The marked text as Ghostty's preedit (cleared when there is none and `clear` says so).
    fn sync_preedit(&self, clear: bool) {
        let Some(surface) = self.surface() else { return };
        let marked = self.ivars().marked.borrow();
        match marked.text() {
            // SAFETY: the live surface on the main thread; the text for the call.
            Some(text) => unsafe { ghostty_surface_preedit(surface, text.as_ptr().cast(), text.len()) },
            // SAFETY: as above, with no text.
            None if clear => unsafe { ghostty_surface_preedit(surface, c"".as_ptr(), 0) },
            None => {}
        }
        runtime::owe();
    }

    /// `translatedEvent`: the event with the modifiers Ghostty translates keys with (its `macos-option-as-alt` and the like).
    fn translated(&self, event: &NSEvent, surface: ghostty_surface_t) -> Retained<NSEvent> {
        let original = flags(event);
        // SAFETY: the live surface on the main thread.
        let translated = keys::appkit_flags(unsafe { ghostty_surface_key_translation_mods(surface, keys::ghostty_mods(original)) });
        let mut finished = original;
        for flag in [keys::SHIFT, keys::CONTROL, keys::OPTION, keys::COMMAND] {
            if translated & flag != 0 {
                finished |= flag;
            } else {
                finished &= !flag;
            }
        }
        let same = event.retain();
        if finished == original {
            return same;
        }
        let modifiers = NSEventModifierFlags(finished as usize);
        let characters = event.charactersByApplyingModifiers(modifiers).unwrap_or_else(|| NSString::from_str(""));
        let ignoring = event.charactersIgnoringModifiers().unwrap_or_else(|| NSString::from_str(""));
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            event.r#type(),
            event.locationInWindow(),
            modifiers,
            event.timestamp(),
            event.windowNumber(),
            None,
            &characters,
            &ignoring,
            event.isARepeat(),
            event.keyCode(),
        )
        .unwrap_or(same)
    }

    /// `TerminalKeyEventHandler.handleKeyDown`.
    fn handle_key_down(&self, event: &NSEvent) {
        let Some(surface) = self.surface() else { return };
        let ivars = self.ivars();
        let action = if event.isARepeat() { GHOSTTY_ACTION_REPEAT } else { GHOSTTY_ACTION_PRESS };
        let translation = self.translated(event, surface);
        *ivars.collected.borrow_mut() = Some(Vec::new());
        ivars.interpreted.set(false);
        let marked_before = ivars.marked.borrow().has_marked_text();
        let layout_before = if marked_before { None } else { keyboard_layout() };
        ivars.last_perform_key.set(None);
        self.interpretKeyEvents(&NSArray::from_retained_slice(std::slice::from_ref(&translation)));
        // The key switched the input source (a layout shortcut): nothing to type.
        if !marked_before && layout_before != keyboard_layout() {
            ivars.collected.borrow_mut().take();
            return;
        }
        self.sync_preedit(marked_before);
        let collected = ivars.collected.borrow_mut().take().filter(|texts| !texts.is_empty());
        if let Some(texts) = collected {
            let mut input = build_key_input(event, action, Some(flags(&translation)));
            for text in texts {
                let Ok(text) = CString::new(text) else { continue };
                input.text = text.as_ptr();
                // SAFETY: the live surface on the main thread; the text lives for the call.
                unsafe { ghostty_surface_key(surface, input) };
            }
            runtime::owe();
            return;
        }
        let composing = ivars.marked.borrow().has_marked_text() || marked_before;
        let include_text = !ivars.interpreted.replace(false);
        let mut input = build_key_input(event, action, Some(flags(&translation)));
        input.composing = composing;
        let text = if include_text { filtered_characters(&translation).filter(|t| !t.is_empty()) } else { None };
        let text = text.and_then(|t| CString::new(t).ok());
        if let Some(text) = &text {
            input.text = text.as_ptr();
        }
        // SAFETY: the live surface on the main thread; the text lives for the call.
        unsafe { ghostty_surface_key(surface, input) };
        runtime::owe();
    }

    /// Whether Ghostty binds this key (then the key is Ghostty's, not the menu's).
    fn key_is_binding(&self, event: &NSEvent, surface: ghostty_surface_t) -> bool {
        let mut input = build_key_input(event, GHOSTTY_ACTION_PRESS, None);
        let text = CString::new(event.characters().map(|c| c.to_string()).unwrap_or_default()).unwrap_or_default();
        input.text = text.as_ptr();
        let mut binding_flags = 0;
        // SAFETY: the live surface on the main thread; the text lives for the call.
        unsafe { ghostty_surface_key_is_binding(surface, input, &mut binding_flags) }
    }

    /// `AppTerminalView.performKeyEquivalent`, with Mighty's ⌘C/⌘V by key position (`ClipboardShortcut`).
    fn handle_key_equivalent(&self, event: &NSEvent) -> bool {
        if event.r#type() != NSEventType::KeyDown || !self.has_keyboard() {
            return false;
        }
        let Some(surface) = self.surface() else { return false };
        // Mighty's `ApplicationCopyRouter`: ⌘C/⌘V first, by the key's characters without modifiers (or its position under a
        // non-Latin input source), before any character-dependent matching; a copy with nothing selected stays the menu's.
        let ignoring = event.charactersIgnoringModifiers().map(|c| c.to_string());
        match keys::clipboard_shortcut(flags(event), ignoring.as_deref(), event.keyCode()) {
            Some(keys::ClipboardKey::Copy) => return self.copy_selection(),
            Some(keys::ClipboardKey::Paste) => return self.binding("paste_from_clipboard"),
            None => {}
        }
        if self.key_is_binding(event, surface) {
            self.handle_key_down(event);
            return true;
        }
        let characters = event.characters().map(|c| c.to_string());
        let modifiers = flags(event);
        let ignoring = event.charactersIgnoringModifiers().map(|c| c.to_string()).unwrap_or_default();
        let equivalent = match ignoring.as_str() {
            "\r" if modifiers & keys::CONTROL != 0 => "\r".to_string(),
            "/" if modifiers & keys::CONTROL != 0 && modifiers & (keys::SHIFT | keys::COMMAND | keys::OPTION) == 0 => "_".to_string(),
            "\r" | "/" => return false,
            _ => {
                if event.timestamp() == 0.0 {
                    return false;
                }
                if modifiers & (keys::COMMAND | keys::CONTROL) == 0 {
                    self.ivars().last_perform_key.set(None);
                    return false;
                }
                if self.ivars().last_perform_key.get() == Some(event.timestamp()) {
                    self.ivars().last_perform_key.set(None);
                    characters.unwrap_or_default()
                } else {
                    self.ivars().last_perform_key.set(Some(event.timestamp()));
                    return false;
                }
            }
        };
        let text = NSString::from_str(&equivalent);
        let translated = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            event.locationInWindow(),
            event.modifierFlags(),
            event.timestamp(),
            event.windowNumber(),
            None,
            &text,
            &text,
            event.isARepeat(),
            event.keyCode(),
        );
        match translated {
            Some(translated) => {
                self.handle_key_down(&translated);
                true
            }
            None => false,
        }
    }

    /// The view's own picture (`snapshotImage`: AppKit's `cacheDisplayInRect`, which holds Ghostty's frame, Step 0's answer) as a PNG
    /// `data:` URL.
    pub fn snapshot(&self) -> Option<String> {
        use objc2_app_kit::NSBitmapImageFileType;
        use objc2_foundation::{NSDataBase64EncodingOptions, NSDictionary};
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return None;
        }
        let representation = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &representation);
        // SAFETY: a PNG of the bitmap with no properties.
        let data = unsafe { representation.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
        let encoded = data.base64EncodedStringWithOptions(NSDataBase64EncodingOptions(0));
        Some(format!("data:image/png;base64,{encoded}"))
    }

    /// The grid Ghostty shows now, row by row (Swift's `readViewportText`; the spike's read-back).
    pub fn viewport_text(&self) -> String {
        let Some(surface) = self.surface() else { return String::new() };
        // SAFETY: the live surface on the main thread; each text is freed by libghostty.
        unsafe {
            let size = ghostty_surface_size(surface);
            let mut lines = Vec::with_capacity(usize::from(size.rows));
            for row in 0..u32::from(size.rows) {
                let point = |x| ghostty_point_s { tag: GHOSTTY_POINT_VIEWPORT, coord: GHOSTTY_POINT_COORD_EXACT, x, y: row };
                let selection = ghostty_selection_s {
                    top_left: point(0),
                    bottom_right: point(u32::from(size.columns).saturating_sub(1)),
                    rectangle: false,
                };
                let mut text: ghostty_text_s = std::mem::zeroed();
                if !ghostty_surface_read_text(surface, selection, &mut text) {
                    lines.push(String::new());
                    continue;
                }
                let line = if text.text.is_null() || text.text_len == 0 {
                    String::new()
                } else {
                    String::from_utf8_lossy(std::slice::from_raw_parts(text.text.cast::<u8>(), text.text_len)).into_owned()
                };
                ghostty_surface_free_text(surface, &mut text);
                lines.push(line);
            }
            lines.join("\n")
        }
    }

    /// Whether `view` is a terminal view (the page's keyboard must go elsewhere when one hides).
    pub fn is_terminal(view: &NSView) -> bool {
        view.isKindOfClass(TerminalView::class())
    }
}

/// The cursor for Ghostty's mouse shape (the AppKit cursors there are; anything else is the arrow). The resize cursors are the ones
/// macOS 14 still has under these names (their newer forms want a direction).
#[allow(deprecated)]
fn cursor_for(shape: i32) -> Retained<NSCursor> {
    match shape {
        GHOSTTY_MOUSE_SHAPE_TEXT | GHOSTTY_MOUSE_SHAPE_CELL => NSCursor::IBeamCursor(),
        GHOSTTY_MOUSE_SHAPE_VERTICAL_TEXT => NSCursor::IBeamCursorForVerticalLayout(),
        GHOSTTY_MOUSE_SHAPE_POINTER => NSCursor::pointingHandCursor(),
        GHOSTTY_MOUSE_SHAPE_CROSSHAIR => NSCursor::crosshairCursor(),
        GHOSTTY_MOUSE_SHAPE_NOT_ALLOWED | GHOSTTY_MOUSE_SHAPE_NO_DROP => NSCursor::operationNotAllowedCursor(),
        GHOSTTY_MOUSE_SHAPE_COL_RESIZE | GHOSTTY_MOUSE_SHAPE_EW_RESIZE => NSCursor::resizeLeftRightCursor(),
        GHOSTTY_MOUSE_SHAPE_ROW_RESIZE | GHOSTTY_MOUSE_SHAPE_NS_RESIZE => NSCursor::resizeUpDownCursor(),
        _ => NSCursor::arrowCursor(),
    }
}

/// Ghostty asked for another mouse shape: the surface's view (while the surface lives) takes its cursor again. Main thread.
pub fn mouse_shape_changed(shared: &Shared) {
    if shared.surface().is_none() {
        return;
    }
    let view = shared.view.load(Ordering::SeqCst);
    if view.is_null() {
        return;
    }
    // SAFETY: the view the surface draws in, alive while its surface is (the surface pointer is cleared before the view goes).
    let view: &TerminalView = unsafe { &*(view as *const TerminalView) };
    if let Some(window) = view.window() {
        window.invalidateCursorRectsForView(view);
    }
}
