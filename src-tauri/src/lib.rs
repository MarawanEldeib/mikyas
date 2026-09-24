use tauri::window::{Effect, EffectsBuilder};
use tauri::{PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

// M0 spike: window effect and WebView2 browser args come from env vars so each
// variant can be launched and measured without rebuilding.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let effect = std::env::var("CUW_EFFECT").unwrap_or_else(|_| "mica".into());
            let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Claude Usage")
                .inner_size(320.0, 232.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .shadow(true)
                .focused(false)
                .visible(false);
            if let Ok(args) = std::env::var("CUW_BROWSER_ARGS") {
                builder = builder.additional_browser_args(&args);
            }
            let window = builder.build()?;
            let chosen = match effect.as_str() {
                "mica" => Some(Effect::Mica),
                "acrylic" => Some(Effect::Acrylic),
                "blur" => Some(Effect::Blur),
                "tabbed" => Some(Effect::Tabbed),
                _ => None,
            };
            if let Some(e) = chosen {
                window.set_effects(EffectsBuilder::new().effect(e).build())?;
            }
            if let (Ok(x), Ok(y)) = (std::env::var("CUW_X"), std::env::var("CUW_Y")) {
                if let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) {
                    window.set_position(PhysicalPosition::new(x, y))?;
                }
            }
            window.show()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Claude Usage Widget");
}
