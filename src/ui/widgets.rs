//! Small helpers shared by every page, so pages stay declarative and consistent.

use gpui_kit::component::dialog::AlertDialog;
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::SelectState;
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex};
use gpui_kit::*;

pub type NumState = Entity<InputState>;
pub type StrSelect = SelectState<SearchableVec<SharedString>>;

/// Numeric input state; render with `NumberInput::new(&state)`.
pub fn num_input(value: f64, min: f64, max: f64, step: f64, window: &mut Window, cx: &mut App) -> NumState {
    cx.new(|cx| {
        InputState::new(window, cx)
            .min(min)
            .max(max)
            .step(step)
            .default_value(fmt_num(value))
    })
}

/// Text input state; render with `Input::new(&state)`.
pub fn text_input(value: &str, placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let (value, placeholder) = (value.to_string(), placeholder.to_string());
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

/// Parsed numeric value (0 when empty or invalid).
pub fn read_num(state: &Entity<InputState>, cx: &App) -> f64 {
    state.read(cx).value().trim().parse::<f64>().unwrap_or(0.0)
}

pub fn read_i(state: &Entity<InputState>, cx: &App) -> i64 {
    read_num(state, cx).round() as i64
}

pub fn read_text(state: &Entity<InputState>, cx: &App) -> String {
    state.read(cx).value().to_string()
}

pub fn set_num(state: &Entity<InputState>, v: f64, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| s.set_value(fmt_num(v), window, cx));
}

pub fn set_text(state: &Entity<InputState>, v: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    let v = v.into();
    state.update(cx, |s, cx| s.set_value(v, window, cx));
}

/// Drop-down of plain strings; render with `Select::new(&state)`.
pub fn select(items: Vec<SharedString>, selected: usize, window: &mut Window, cx: &mut App) -> Entity<StrSelect> {
    cx.new(|cx| {
        let sel = (!items.is_empty()).then(|| IndexPath::new(selected.min(items.len() - 1)));
        SelectState::new(SearchableVec::new(items), sel, window, cx)
    })
}

pub fn selected_index(state: &Entity<StrSelect>, cx: &App) -> usize {
    state.read(cx).selected_index(cx).map(|ix| ix.row).unwrap_or(0)
}

pub fn selected_text(state: &Entity<StrSelect>, cx: &App) -> Option<String> {
    state.read(cx).selected_value().map(|s| s.to_string())
}

pub fn set_items(state: &Entity<StrSelect>, items: Vec<SharedString>, selected: usize, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| {
        let n = items.len();
        s.set_items(SearchableVec::new(items), window, cx);
        if n > 0 {
            s.set_selected_index(Some(IndexPath::new(selected.min(n - 1))), window, cx);
        }
    });
}

/// Titled panel used to group controls, like the original QGroupBox sections.
pub fn section(title: impl Into<SharedString>) -> GroupBox {
    GroupBox::new().outline().title(title.into())
}

/// `label  [control]` row.
pub fn field(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .items_center()
        .child(div().min_w(px(84.)).text_sm().text_color(cx.theme().muted_foreground).child(label.into()))
        .child(div().flex_1().min_w(px(120.)).child(control))
}

/// OK/Cancel confirmation; `on_ok` runs only when confirmed.
pub fn confirm(
    title: impl Into<SharedString>,
    message: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let (title, message) = (title.into(), message.into());
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_alert_dialog(cx, move |d: AlertDialog, _, _| {
        let on_ok = on_ok.clone();
        d.confirm().title(title.clone()).description(message.clone()).on_ok(move |_, window, cx| {
            on_ok(window, cx);
            true
        })
    });
}

pub fn notify_info(msg: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::info(msg.into()), cx);
}

pub fn notify_error(msg: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::error(msg.into()), cx);
}
