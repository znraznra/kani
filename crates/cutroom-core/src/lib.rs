//! Toolkit-independent application logic for Kani: the editing session,
//! timeline interaction, settings, the new-project form, and home-screen
//! helpers. The GTK front end in `cutroom-gtk` is a thin layer over this.

pub mod clip_menu;
pub mod credits;
pub mod geom;
pub mod home_logic;
pub mod new_project;
pub mod paths;
pub mod settings;
pub mod shortcuts;
pub mod state;
pub mod text_styles;
pub mod timeline_logic;
pub mod transform_box;
pub mod ui_state;
pub mod view;
pub mod waveform;
