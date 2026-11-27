//! The workbench's colours by VS Code's colour ids: the theme's value, else the next id VS Code falls back to, else
//! VS Code's own default for a dark or a light theme (its colour registry's), so a theme that names few colours
//! looks as it does in VS Code.

use floem::peniko::Color;

/// A colour written as VS Code writes them: #rgb, #rgba, #rrggbb or #rrggbbaa.
pub fn hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    let n = |i: usize, w: usize| u8::from_str_radix(&h[i..i + w], 16).ok();
    let one = |i: usize| n(i, 1).map(|v| v * 17);
    match h.len() {
        3 => Some(Color::rgb8(one(0)?, one(1)?, one(2)?)),
        4 => Some(Color::rgba8(one(0)?, one(1)?, one(2)?, one(3)?)),
        6 => Some(Color::rgb8(n(0, 2)?, n(2, 2)?, n(4, 2)?)),
        8 => Some(Color::rgba8(n(0, 2)?, n(2, 2)?, n(4, 2)?, n(6, 2)?)),
        _ => None,
    }
}

macro_rules! colors {
    ($($field:ident: [$($id:literal),+], $dark:literal, $light:literal;)+) => {
        /// Every colour the workbench draws with.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Colors {
            pub dark: bool,
            $(pub $field: Color,)+
        }

        impl Colors {
            pub fn of(theme: &ca_themes::Theme) -> Colors {
                let dark = theme.is_dark();
                let colors = &theme.json["colors"];
                let pick = |ids: &[&str], d: &str, l: &str| {
                    ids.iter()
                        .find_map(|id| colors[*id].as_str().and_then(hex))
                        .or_else(|| hex(if dark { d } else { l }))
                        .unwrap_or(Color::rgb8(0x80, 0x80, 0x80))
                };
                Colors { dark, $($field: pick(&[$($id),+], $dark, $light),)+ }
            }
        }
    };
}

colors! {
    fg: ["foreground"], "#CCCCCC", "#616161";
    desc: ["descriptionForeground"], "#CCCCCCB3", "#717171";
    icon: ["icon.foreground"], "#C5C5C5", "#424242";
    focus: ["focusBorder"], "#007FD4", "#0090F1";
    error: ["errorForeground"], "#F48771", "#A1260D";
    warning: ["editorWarning.foreground"], "#CCA700", "#BF8803";
    link: ["textLink.foreground"], "#3794FF", "#006AB1";
    shadow: ["widget.shadow"], "#0000005C", "#00000029";
    widget: ["editorWidget.background"], "#252526", "#F3F3F3";
    widget_border: ["editorWidget.border", "widget.border"], "#454545", "#C8C8C8";
    code: ["textCodeBlock.background"], "#0A0A0A66", "#DCDCDC66";
    preformat: ["textPreformat.foreground"], "#D7BA7D", "#A31515";
    preformat_bg: ["textPreformat.background"], "#FFFFFF1A", "#0000001A";
    quote_border: ["textBlockQuote.border"], "#007ACC80", "#007ACC80";
    ok: ["testing.iconPassed", "terminal.ansiGreen"], "#73C991", "#388A34";

    title_bg: ["titleBar.activeBackground"], "#3C3C3C", "#DDDDDD";
    title_fg: ["titleBar.activeForeground"], "#CCCCCC", "#333333";
    title_border: ["titleBar.border", "contrastBorder"], "#00000000", "#00000000";
    command_bg: ["commandCenter.background"], "#FFFFFF0D", "#0000000D";
    command_fg: ["commandCenter.foreground", "titleBar.activeForeground"], "#CCCCCC", "#333333";
    command_border: ["commandCenter.border"], "#CCCCCC33", "#33333333";
    command_hover: ["commandCenter.activeBackground"], "#FFFFFF14", "#00000014";
    toolbar_hover: ["toolbar.hoverBackground"], "#5A5D5E50", "#B8B8B850";

    activity_bg: ["activityBar.background"], "#333333", "#2C2C2C";
    activity_fg: ["activityBar.foreground"], "#FFFFFF", "#FFFFFF";
    activity_inactive: ["activityBar.inactiveForeground"], "#FFFFFF66", "#FFFFFF66";
    activity_active_border: ["activityBar.activeBorder", "activityBar.foreground"], "#FFFFFF", "#FFFFFF";
    activity_border: ["activityBar.border", "contrastBorder"], "#00000000", "#00000000";
    badge_bg: ["activityBarBadge.background"], "#007ACC", "#007ACC";
    badge_fg: ["activityBarBadge.foreground"], "#FFFFFF", "#FFFFFF";

    side_bg: ["sideBar.background"], "#252526", "#F3F3F3";
    side_fg: ["sideBar.foreground", "foreground"], "#CCCCCC", "#616161";
    side_border: ["sideBar.border", "contrastBorder"], "#00000000", "#00000000";
    side_title: ["sideBarTitle.foreground", "sideBar.foreground", "foreground"], "#BBBBBB", "#6F6F6F";
    section_bg: ["sideBarSectionHeader.background"], "#00000000", "#00000000";
    section_fg: ["sideBarSectionHeader.foreground", "sideBar.foreground", "foreground"], "#CCCCCC", "#616161";
    section_border: ["sideBarSectionHeader.border", "contrastBorder"], "#CCCCCC33", "#61616133";

    list_hover: ["list.hoverBackground"], "#2A2D2E", "#E8E8E8";
    list_selected: ["list.activeSelectionBackground"], "#04395E", "#0060C0";
    list_selected_fg: ["list.activeSelectionForeground", "foreground"], "#FFFFFF", "#FFFFFF";
    list_inactive: ["list.inactiveSelectionBackground"], "#37373D", "#E4E6F1";
    list_outline: ["list.focusAndSelectionOutline", "list.focusOutline", "focusBorder"], "#007FD4", "#0090F1";

    editor_bg: ["editor.background"], "#1E1E1E", "#FFFFFF";
    editor_fg: ["editor.foreground"], "#D4D4D4", "#000000";
    line_number: ["editorLineNumber.foreground"], "#858585", "#237893";
    line_number_active: ["editorLineNumber.activeForeground"], "#C6C6C6", "#0B216F";
    line_highlight: ["editor.lineHighlightBorder", "editor.lineHighlightBackground"], "#282828", "#EEEEEE";
    selection: ["editor.selectionBackground"], "#264F78", "#ADD6FF";
    cursor: ["editorCursor.foreground"], "#AEAFAD", "#000000";
    indent_guide: ["editorIndentGuide.background1", "editorIndentGuide.background"], "#404040", "#D3D3D3";
    whitespace: ["editorWhitespace.foreground"], "#E3E4E229", "#33333333";
    gutter_bg: ["editorGutter.background", "editor.background"], "#1E1E1E", "#FFFFFF";
    problem_error: ["editorError.foreground"], "#F14C4C", "#E51400";
    problem_info: ["editorInfo.foreground"], "#3794FF", "#1A85FF";
    problem_hint: ["editorHint.foreground"], "#EEEEEEB3", "#6C6C6C";
    code_lens: ["editorCodeLens.foreground"], "#999999", "#919191";
    ghost_text: ["editorGhostText.foreground"], "#FFFFFF56", "#00000077";
    inlay_fg: ["editorInlayHint.foreground"], "#969696", "#969696";
    inlay_bg: ["editorInlayHint.background"], "#4D4D4D1A", "#C4C4C41A";
    link_active: ["editorLink.activeForeground"], "#4E94CE", "#0000FF";
    hover_bg: ["editorHoverWidget.background", "editorWidget.background"], "#252526", "#F3F3F3";
    hover_fg: ["editorHoverWidget.foreground", "editorWidget.foreground", "foreground"], "#CCCCCC", "#616161";
    hover_border: ["editorHoverWidget.border", "editorWidget.border"], "#454545", "#C8C8C8";
    ruler_border: ["editorOverviewRuler.border"], "#7F7F7F4D", "#7F7F7F4D";
    ruler_error: ["editorOverviewRuler.errorForeground"], "#FF1212B3", "#FF1212B3";
    ruler_warning: ["editorOverviewRuler.warningForeground", "editorWarning.foreground"], "#CCA700", "#BF8803";
    ruler_info: ["editorOverviewRuler.infoForeground", "editorInfo.foreground"], "#3794FF", "#1A85FF";

    tabs_bg: ["editorGroupHeader.tabsBackground"], "#252526", "#F3F3F3";
    tabs_border: ["editorGroupHeader.tabsBorder"], "#00000000", "#00000000";
    tab_active_bg: ["tab.activeBackground", "editor.background"], "#1E1E1E", "#FFFFFF";
    tab_active_fg: ["tab.activeForeground"], "#FFFFFF", "#333333";
    tab_inactive_bg: ["tab.inactiveBackground"], "#2D2D2D", "#ECECEC";
    tab_inactive_fg: ["tab.inactiveForeground"], "#FFFFFF80", "#33333380";
    tab_border: ["tab.border"], "#252526", "#F3F3F3";
    tab_top: ["tab.activeBorderTop"], "#00000000", "#00000000";
    tab_hover: ["tab.hoverBackground", "tab.inactiveBackground"], "#2D2D2D", "#ECECEC";
    group_border: ["editorGroup.border"], "#444444", "#E7E7E7";
    breadcrumb: ["breadcrumb.foreground"], "#CCCCCCCC", "#616161CC";
    watermark: ["editorWatermark.foreground"], "#CCCCCC99", "#61616199";

    panel_bg: ["panel.background", "editor.background"], "#1E1E1E", "#FFFFFF";
    panel_border: ["panel.border"], "#80808059", "#80808059";
    panel_title: ["panelTitle.activeForeground"], "#E7E7E7", "#424242";
    panel_title_inactive: ["panelTitle.inactiveForeground"], "#E7E7E799", "#42424275";
    panel_title_border: ["panelTitle.activeBorder", "panelTitle.activeForeground"], "#E7E7E7", "#424242";

    status_bg: ["statusBar.background"], "#007ACC", "#007ACC";
    status_fg: ["statusBar.foreground"], "#FFFFFF", "#FFFFFF";
    status_border: ["statusBar.border", "contrastBorder"], "#00000000", "#00000000";
    status_hover: ["statusBarItem.hoverBackground"], "#FFFFFF1F", "#FFFFFF1F";
    remote_bg: ["statusBarItem.remoteBackground"], "#16825D", "#16825D";
    remote_fg: ["statusBarItem.remoteForeground"], "#FFFFFF", "#FFFFFF";

    input_bg: ["input.background"], "#3C3C3C", "#FFFFFF";
    input_fg: ["input.foreground", "foreground"], "#CCCCCC", "#616161";
    input_border: ["input.border"], "#00000000", "#00000000";
    placeholder: ["input.placeholderForeground"], "#A6A6A6", "#767676";
    input_error: ["inputValidation.errorBorder"], "#BE1100", "#BE1100";
    input_error_bg: ["inputValidation.errorBackground"], "#5A1D1D", "#F2DEDE";

    button_bg: ["button.background"], "#0E639C", "#007ACC";
    button_fg: ["button.foreground"], "#FFFFFF", "#FFFFFF";
    button_hover: ["button.hoverBackground"], "#1177BB", "#0062A3";
    button2_bg: ["button.secondaryBackground"], "#3A3D41", "#5F6A79";
    button2_fg: ["button.secondaryForeground"], "#FFFFFF", "#FFFFFF";
    button2_hover: ["button.secondaryHoverBackground"], "#45494E", "#4C5561";

    scrollbar: ["scrollbarSlider.background"], "#79797966", "#64646466";
    scrollbar_hover: ["scrollbarSlider.hoverBackground"], "#646464B3", "#646464B3";
    sash: ["sash.hoverBorder", "focusBorder"], "#007FD4", "#0090F1";
    drop_bg: ["sideBar.dropBackground", "panel.dropBackground", "editorGroup.dropBackground"], "#53595D80", "#2677CB2D";

    menu_bg: ["menu.background", "dropdown.background"], "#252526", "#FFFFFF";
    menu_fg: ["menu.foreground", "dropdown.foreground", "foreground"], "#CCCCCC", "#616161";
    menu_selected: ["menu.selectionBackground", "list.activeSelectionBackground"], "#04395E", "#0060C0";
    menu_selected_fg: ["menu.selectionForeground", "list.activeSelectionForeground"], "#FFFFFF", "#FFFFFF";
    menu_border: ["menu.border", "widget.border"], "#454545", "#CECECE";
    menu_separator: ["menu.separatorBackground"], "#606060", "#D4D4D4";

    quick_bg: ["quickInput.background", "editorWidget.background"], "#252526", "#F3F3F3";
    quick_fg: ["quickInput.foreground", "editorWidget.foreground", "foreground"], "#CCCCCC", "#616161";
    quick_border: ["widget.border", "contrastBorder", "editorWidget.border"], "#454545", "#C8C8C8";
    quick_focus_bg: ["quickInputList.focusBackground", "quickInput.list.focusBackground", "list.activeSelectionBackground"], "#04395E", "#0060C0";
    quick_focus_fg: ["quickInputList.focusForeground", "list.activeSelectionForeground"], "#FFFFFF", "#FFFFFF";
    highlight: ["list.highlightForeground"], "#2AAAFF", "#0066BF";
    highlight_focus: ["list.focusHighlightForeground", "list.highlightForeground"], "#2AAAFF", "#BBE7FF";
    key_bg: ["keybindingLabel.background"], "#8080802B", "#DDDDDD66";
    key_fg: ["keybindingLabel.foreground"], "#CCCCCC", "#555555";
    key_border: ["keybindingLabel.border"], "#33333399", "#CCCCCC66";
    symbol_class: ["symbolIcon.classForeground"], "#EE9D28", "#D67E00";
    symbol_enum: ["symbolIcon.enumeratorForeground"], "#EE9D28", "#D67E00";
    symbol_function: ["symbolIcon.functionForeground"], "#B180D7", "#652D90";
    symbol_method: ["symbolIcon.methodForeground"], "#B180D7", "#652D90";
    symbol_interface: ["symbolIcon.interfaceForeground"], "#75BEFF", "#007ACC";
    symbol_variable: ["symbolIcon.variableForeground"], "#75BEFF", "#007ACC";
    symbol_field: ["symbolIcon.fieldForeground"], "#75BEFF", "#007ACC";
    symbol_struct: ["symbolIcon.structForeground", "foreground"], "#CCCCCC", "#616161";
    symbol_namespace: ["symbolIcon.namespaceForeground", "foreground"], "#CCCCCC", "#616161";
    symbol_constant: ["symbolIcon.constantForeground", "foreground"], "#CCCCCC", "#616161";
    symbol_string: ["symbolIcon.stringForeground", "foreground"], "#CCCCCC", "#616161";

    notice_bg: ["inputValidation.warningBackground"], "#352A05", "#F6F5D2";
    notice_border: ["inputValidation.warningBorder"], "#B89500", "#B89500";

    chat_request: ["chat.requestBubbleBackground", "chat.requestBackground", "list.hoverBackground"], "#2A2D2E", "#F2F2F2";
    terminal_bg: ["terminal.background", "panel.background"], "#1E1E1E", "#FFFFFF";
    terminal_fg: ["terminal.foreground"], "#CCCCCC", "#333333";
    terminal_selection: ["terminal.selectionBackground", "editor.selectionBackground"], "#264F78", "#ADD6FF";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_as_vs_code_writes_them() {
        assert_eq!(hex("#fff"), Some(Color::rgb8(255, 255, 255)));
        assert_eq!(hex("#0078D4"), Some(Color::rgb8(0, 0x78, 0xD4)));
        assert_eq!(hex("#FFFFFF12"), Some(Color::rgba8(255, 255, 255, 0x12)));
        assert_eq!(hex("red"), None);
    }

    #[test]
    fn a_theme_that_names_few_colours_gets_vs_codes_own() {
        let all = ca_themes::built_in();
        let modern = Colors::of(&ca_themes::chosen(&all, "dark-modern", true));
        assert_eq!(modern.activity_bg, Color::rgb8(0x18, 0x18, 0x18));
        assert_eq!(modern.tab_top, Color::rgb8(0, 0x78, 0xD4));
        let bare = ca_themes::Theme { id: "x".into(), json: serde_json::json!({"type": "dark", "colors": {}}) };
        assert_eq!(Colors::of(&bare).status_bg, Color::rgb8(0, 0x7A, 0xCC));
    }
}
