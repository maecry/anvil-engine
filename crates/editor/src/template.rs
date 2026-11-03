//! The "New project" template: manifest, `assets/` and a default scene (camera, sun, ground).
//! Everything is generated here; nothing is copied from elsewhere.

use std::path::Path;

use assets::AssetId;
use foundation::math::{EulerRot, Quat, Vec3};
use runtime::{PROJECT_FILE, Project};
use scene::{Camera, EntityData, EntityUuid, Light, LightKind, MeshRenderer, SceneFile, Transform};

use crate::{EditorError, FlyCamera};

/// Startup scene of a new project, relative to the project directory.
pub const DEFAULT_SCENE: &str = "assets/scenes/main.scene";

/// The default scene: a camera looking at the origin, a shadow-casting sun and a ground slab
/// (the renderer's built-in cube, flattened, so the template needs no mesh asset).
pub fn default_scene() -> SceneFile {
    let entity = |name: &str, transform: Transform| EntityData {
        uuid: EntityUuid(AssetId::random().0),
        name: Some(name.to_owned()),
        parent: None,
        transform: Some(transform),
        mesh_renderer: None,
        light: None,
        camera: None,
    };
    let eye = Vec3::new(6.0, 4.0, 8.0);
    let camera = EntityData {
        camera: Some(Camera::default()),
        ..entity(
            "Camera",
            Transform {
                translation: eye,
                rotation: FlyCamera::looking_at(eye, Vec3::ZERO).rotation(),
                scale: Vec3::ONE,
            },
        )
    };
    let sun = EntityData {
        light: Some(Light {
            kind: LightKind::Directional,
            color: [1.0, 0.96, 0.9],
            intensity: 3.0,
            cast_shadows: true,
            ..Light::default()
        }),
        ..entity(
            "Sun",
            Transform {
                rotation: Quat::from_euler(EulerRot::YXZ, 0.6, -0.9, 0.0),
                ..Transform::IDENTITY
            },
        )
    };
    let ground = EntityData {
        mesh_renderer: Some(MeshRenderer {
            mesh: AssetId::NIL,
            material: AssetId::NIL,
        }),
        ..entity(
            "Ground",
            Transform {
                translation: Vec3::new(0.0, -0.1, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::new(30.0, 0.2, 30.0),
            },
        )
    };
    SceneFile {
        version: assets::SCENE_VERSION,
        entities: vec![camera, sun, ground],
    }
}

/// Creates the project in `dir` (which must not already hold one).
///
/// # Errors
/// [`EditorError::Command`] when `dir` already is a project, [`EditorError::Io`] on write failure.
pub fn create_project(dir: &Path) -> Result<(), EditorError> {
    if dir.join(PROJECT_FILE).exists() {
        return Err(EditorError::cmd(format!(
            "{} already contains a project",
            dir.display()
        )));
    }
    let name = dir
        .file_name()
        .map_or("Untitled".into(), |n| n.to_string_lossy().into_owned());
    for sub in ["scenes", "models", "materials", "textures"] {
        let d = dir.join("assets").join(sub);
        std::fs::create_dir_all(&d).map_err(|e| EditorError::io(&d, &e))?;
    }
    Project::new(&name, DEFAULT_SCENE).save(dir)?;
    let scene = dir.join(DEFAULT_SCENE);
    std::fs::write(&scene, default_scene().to_ron()).map_err(|e| EditorError::io(&scene, &e))?;
    let ignore = dir.join(".gitignore");
    std::fs::write(&ignore, ".anvil/\n").map_err(|e| EditorError::io(&ignore, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_a_valid_project_with_camera_sun_and_ground() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("proj");
        create_project(&p).unwrap();
        assert!(create_project(&p).is_err(), "second create must refuse");
        let project = Project::load(&p).unwrap();
        let text = std::fs::read_to_string(p.join(&project.startup_scene)).unwrap();
        let scene = SceneFile::from_ron(&text).unwrap();
        let has = |f: fn(&EntityData) -> bool| scene.entities.iter().any(f);
        assert!(has(|e| e.camera.is_some()));
        assert!(has(|e| e.light.is_some()));
        assert!(has(|e| e.mesh_renderer.is_some()));
        scene.spawn_into(&mut scene::World::new()).unwrap();
    }
}
