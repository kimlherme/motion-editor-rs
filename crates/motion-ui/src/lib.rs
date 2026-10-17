//! Native egui editor shell for the first project and still-image workflow.
use eframe::egui;
use motion_core::{project::{Interpolation, MediaAsset, MediaKind, StableId, Vec2}, time::{FrameRate, Tick}};
use motion_engine::EditorSession;
use motion_media::{FrameProvider, MediaError, ProvidedFrame, StillImageProvider};
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

#[derive(Debug)]
pub struct UiError(pub String);
impl std::fmt::Display for UiError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) } }
impl std::error::Error for UiError {}

pub fn run() -> Result<(), UiError> {
    let options = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1120.0, 760.0]), ..Default::default() };
    eframe::run_native("Editor de movimento", options, Box::new(|_cc| Ok(Box::new(EditorApp::default()))))
        .map_err(|e| UiError(e.to_string()))
}

/// Small still provider for the shell. Video decode remains an explicit backend boundary.
#[derive(Default)]
struct StillProvider { cache: Mutex<HashMap<StableId, StillImageProvider>> }
impl FrameProvider for StillProvider {
    fn frame_at(&self, asset: &MediaAsset, source_time: Tick) -> Result<Option<ProvidedFrame>, MediaError> {
        if asset.kind != MediaKind::Image { return Err(MediaError::UnsupportedBackend("video frame decoding (no video backend is configured)")); }
        let cache = {
            let mut cache = self.cache.lock().map_err(|_| MediaError::UnsupportedBackend("still provider cache"))?;
            if !cache.contains_key(&asset.id) {
                let frame = motion_media::load_still(&asset.locator)?;
                cache.insert(asset.id, StillImageProvider::new(asset.id, frame));
            }
            cache
        };
        cache.get(&asset.id).expect("provider inserted for this asset").frame_at(asset, source_time)
    }
}

struct EditorApp {
    session: Option<EditorSession>,
    project_name: String,
    width: u32,
    height: u32,
    path: String,
    image_path: String,
    notice: String,
    preview: Option<egui::TextureHandle>,
    provider: StillProvider,
    curve: Interpolation,
}
impl Default for EditorApp {
    fn default() -> Self { Self { session: None, project_name: "Sem título".into(), width: 1080, height: 1920, path: String::new(), image_path: String::new(), notice: "Crie um projeto para começar".into(), preview: None, provider: StillProvider::default(), curve: Interpolation::Linear } }
}
impl EditorApp {
    fn create(&mut self) {
        match EditorSession::new_project(self.project_name.clone(), self.width, self.height, FrameRate::new(30, 1).unwrap()) {
            Ok(session) => { self.session = Some(session); self.preview = None; self.notice = "Projeto criado".into(); }
            Err(error) => self.notice = error.to_string(),
        }
    }
    fn save(&mut self) {
        self.notice = match self.session.as_ref() { Some(s) => s.save(&self.path).map(|_| "Projeto salvo".to_string()).unwrap_or_else(|e| e.to_string()), None => "Crie ou abra um projeto primeiro".into() };
    }
    fn open(&mut self) {
        match EditorSession::open(&self.path) { Ok(s) => { self.session = Some(s); self.preview = None; self.notice = "Projeto aberto".into(); }, Err(e) => self.notice = e.to_string() }
    }
    fn render_preview(&mut self, ctx: &egui::Context) {
        let Some(session) = self.session.as_ref() else { self.notice = "Crie ou abra um projeto primeiro".into(); return; };
        match session.preview(&self.provider) {
            Ok(frame) => {
                let image = egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], frame.pixels());
                self.preview = Some(ctx.load_texture("project-preview", image, egui::TextureOptions::LINEAR));
                self.notice = format!("Prévia renderizada no tick {}", session.playhead().0);
            }
            Err(error) => self.notice = error.to_string(),
        }
    }
}
impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Projeto"); ui.text_edit_singleline(&mut self.project_name);
                ui.add(egui::DragValue::new(&mut self.width).range(16..=8192).prefix("Largura "));
                ui.add(egui::DragValue::new(&mut self.height).range(16..=8192).prefix("Altura "));
                if ui.button("Novo").clicked() { self.create(); }
                ui.separator(); ui.label("Arquivo .mproj"); ui.text_edit_singleline(&mut self.path);
                if ui.button("Abrir").clicked() { self.open(); }
                if ui.button("Salvar").clicked() { self.save(); }
            });
        });
        let timeline_rows = self.session.as_ref().map(|s| s.project().tracks.iter().map(|t| (t.name.clone(), t.clips.iter().map(|c| (c.id, c.duration)).collect::<Vec<_>>())).collect::<Vec<_>>()).unwrap_or_default();
        let selected_clip = self.session.as_ref().and_then(EditorSession::selected_clip);
        let selected_source = self.session.as_ref().and_then(|s| selected_clip.and_then(|id| s.project().tracks.iter().flat_map(|t| &t.clips).find(|c| c.id == id).map(|c| (c.asset_id, c.source_in, c.duration))));
        egui::SidePanel::left("media").default_width(270.0).show(ctx, |ui| {
            ui.heading("Mídia"); ui.label("Caminho de imagem PNG/JPEG"); ui.text_edit_singleline(&mut self.image_path);
            if ui.button("Importar imagem e criar clipe").clicked() {
                self.notice = match self.session.as_mut() { Some(s) => s.import_image(PathBuf::from(&self.image_path)).map(|_| "Imagem importada e clipe adicionado".to_string()).unwrap_or_else(|e| e.to_string()), None => "Crie ou abra um projeto primeiro".into() };
                self.preview = None;
            }
            ui.small("Vídeo/áudio e exportação MP4/H.264 aguardam backend aprovado.");
            ui.separator(); ui.heading("Linha do tempo");
            for (track_name, clips) in &timeline_rows {
                    ui.label(format!("{} · {} clipes", track_name, clips.len()));
                    for (id, duration) in clips {
                        let label = format!("Clipe {} · {} ticks", id, duration.0);
                        if ui.selectable_label(selected_clip == Some(*id), label).clicked() {
                            if let Some(session) = self.session.as_mut() { let _ = session.select_clip(*id); }
                        }
                    }
            }
            if ui.add_enabled(selected_source.is_some(), egui::Button::new("Duplicar clipe no playhead")).clicked() {
                if let (Some(session), Some((asset, source_in, duration))) = (self.session.as_mut(), selected_source) {
                    let at = session.playhead();
                    match session.add_clip(asset, at, source_in, duration) { Ok(id) => { let _ = session.select_clip(id); self.notice = "Clipe adicionado".into(); self.preview = None; }, Err(e) => self.notice = e.to_string() }
                }
            }
        });
        let has_project = self.session.is_some();
        let current_playhead = self.session.as_ref().map(EditorSession::playhead).unwrap_or(Tick::ZERO);
        let mut render_clicked = false;
        let selected_transform = self.session.as_ref().and_then(|s| s.selected_transform().ok());
        let mut position_edit = None;
        let mut scale_edit = None;
        let mut rotation_edit = None;
        let mut opacity_edit = None;
        let mut playhead_changed = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Prévia");
            if has_project {
                ui.horizontal(|ui| {
                    ui.label("Playhead (ticks)");
                    let mut t = current_playhead.0;
                    if ui.add(egui::DragValue::new(&mut t).speed(1_000_000.0)).changed() {
                        if let Some(s) = self.session.as_mut() { s.set_playhead(Tick(t)); }
                        self.preview = None;
                        playhead_changed = true;
                    }
                });
                if ui.button("Renderizar prévia").clicked() { render_clicked = true; }
                if let Some(texture) = &self.preview { ui.add(egui::Image::new(texture).max_size(egui::vec2(360.0, 560.0))); }
                else { ui.centered_and_justified(|ui| { ui.label("Importe uma imagem PNG ou JPEG e renderize a prévia"); }); }
                ui.separator();
                ui.heading("Propriedades no playhead");
                if playhead_changed {
                    ui.label("Atualizando propriedades no novo tempo…");
                } else if let Some(mut transform) = selected_transform {
                    egui::ComboBox::from_label("Curva do próximo keyframe").selected_text(interpolation_label(self.curve)).show_ui(ui, |ui| {
                        for curve in [Interpolation::Linear, Interpolation::EaseIn, Interpolation::EaseOut, Interpolation::EaseInOut] {
                            ui.selectable_value(&mut self.curve, curve, interpolation_label(curve));
                        }
                    });
                    egui::Grid::new("transform-inspector").num_columns(2).show(ui, |ui| {
                        ui.label("Posição X / Y");
                        let changed = ui.add(egui::DragValue::new(&mut transform.position.x).speed(1.0)).changed() | ui.add(egui::DragValue::new(&mut transform.position.y).speed(1.0)).changed();
                        if changed { position_edit = Some(transform.position); } ui.end_row();
                        ui.label("Escala X / Y");
                        let changed = ui.add(egui::DragValue::new(&mut transform.scale.x).speed(0.01)).changed() | ui.add(egui::DragValue::new(&mut transform.scale.y).speed(0.01)).changed();
                        if changed { scale_edit = Some(transform.scale); } ui.end_row();
                        ui.label("Rotação (graus)");
                        if ui.add(egui::DragValue::new(&mut transform.rotation_degrees).speed(1.0)).changed() { rotation_edit = Some(transform.rotation_degrees); } ui.end_row();
                        ui.label("Opacidade");
                        if ui.add(egui::DragValue::new(&mut transform.opacity).speed(0.01).range(0.0..=1.0)).changed() { opacity_edit = Some(transform.opacity); } ui.end_row();
                    });
                } else if selected_clip.is_some() { ui.label("O playhead precisa estar dentro do clipe selecionado."); }
                else { ui.label("Selecione um clipe para editar suas propriedades."); }
            } else { ui.centered_and_justified(|ui| { ui.label("Crie um projeto ou abra um arquivo .mproj"); }); }
        });
        let curve = self.curve;
        if let Some(session) = self.session.as_mut() {
            let outcomes = [
                position_edit.map(|value| session.set_selected_position_keyframe(value, curve)),
                scale_edit.map(|value| session.set_selected_scale_keyframe(value, curve)),
                rotation_edit.map(|value| session.set_selected_rotation_keyframe(value, curve)),
                opacity_edit.map(|value| session.set_selected_opacity_keyframe(value, curve)),
            ];
            for outcome in outcomes.into_iter().flatten() {
                match outcome { Ok(()) => { self.notice = "Keyframe atualizado".into(); self.preview = None; }, Err(error) => self.notice = error.to_string() }
            }
        }
        if render_clicked { self.render_preview(ctx); }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| { ui.label(&self.notice); });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn app_starts_without_project_with_portrait_canvas() { let app = EditorApp::default(); assert!(app.session.is_none()); assert_eq!((app.width, app.height), (1080, 1920)); }
}

fn interpolation_label(curve: Interpolation) -> &'static str {
    match curve { Interpolation::Linear => "Linear", Interpolation::EaseIn => "Entrada suave", Interpolation::EaseOut => "Saída suave", Interpolation::EaseInOut => "Entrada e saída" }
}
