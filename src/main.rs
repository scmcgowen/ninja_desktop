mod app;
mod diff_patch;
mod protocol;
mod session;
mod settings;
mod storage;
mod token;
mod ui;

use std::sync::Arc;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // rustls 0.23 no longer picks a crypto provider by default; install ring
    // once, at startup, before any TLS handshake runs on the WS task.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring provider");

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("samari-ws")
            .build()
            .expect("build tokio runtime"),
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([700.0, 480.0])
            .with_title("Ninja Catcher"),
        ..Default::default()
    };

    eframe::run_native(
        "Ninja Catcher",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(app::AppState::new(runtime.clone(), cc.egui_ctx.clone())))
        }),
    )
}
