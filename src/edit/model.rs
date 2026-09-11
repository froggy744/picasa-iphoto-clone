#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

impl CropRect {
    pub fn normalized(self) -> Self {
        let mut left = self.left.clamp(0.0, 1.0);
        let mut right = self.right.clamp(0.0, 1.0);
        let mut top = self.top.clamp(0.0, 1.0);
        let mut bottom = self.bottom.clamp(0.0, 1.0);
        if right < left {
            std::mem::swap(&mut left, &mut right);
        }
        if bottom < top {
            std::mem::swap(&mut top, &mut bottom);
        }
        if right - left < 0.01 {
            right = (left + 0.01).min(1.0);
            left = (right - 0.01).max(0.0);
        }
        if bottom - top < 0.01 {
            bottom = (top + 0.01).min(1.0);
            top = (bottom - 0.01).max(0.0);
        }
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn is_full(self) -> bool {
        (self.left.abs() < 0.0001)
            && (self.top.abs() < 0.0001)
            && ((self.right - 1.0).abs() < 0.0001)
            && ((self.bottom - 1.0).abs() < 0.0001)
    }

    /// Compose a crop selected inside the already-cropped image with the
    /// persisted crop. This makes repeated crop operations predictable while
    /// keeping every crop normalized to the original edited frame.
    pub fn compose(self, child: CropRect) -> CropRect {
        let parent = self.normalized();
        let child = child.normalized();
        let width = parent.right - parent.left;
        let height = parent.bottom - parent.top;
        CropRect {
            left: parent.left + child.left * width,
            top: parent.top + child.top * height,
            right: parent.left + child.right * width,
            bottom: parent.top + child.bottom * height,
        }
        .normalized()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditRecipe {
    pub crop: CropRect,
    pub straighten: f32,
    pub exposure: f32,
    pub fill_light: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temperature: f32,
    pub saturation: f32,
    pub auto_contrast: bool,
    pub auto_color: bool,
    pub black_white: bool,
    pub sepia: bool,
    pub sharpen: f32,
}

impl Default for EditRecipe {
    fn default() -> Self {
        Self {
            crop: CropRect::default(),
            straighten: 0.0,
            exposure: 0.0,
            fill_light: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            temperature: 0.0,
            saturation: 0.0,
            auto_contrast: false,
            auto_color: false,
            black_white: false,
            sepia: false,
            sharpen: 0.0,
        }
    }
}

impl EditRecipe {
    pub fn decode(value: &str) -> Self {
        if value.trim().is_empty() {
            return Self::default();
        }
        let mut recipe = Self::default();
        for part in value.split('|') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let number = || value.parse::<f32>().ok();
            let flag = || matches!(value, "1" | "true" | "yes" | "on");
            match key {
                "crop" => {
                    let values = value
                        .split(',')
                        .filter_map(|item| item.parse::<f32>().ok())
                        .collect::<Vec<_>>();
                    if values.len() == 4 {
                        recipe.crop = CropRect {
                            left: values[0],
                            top: values[1],
                            right: values[2],
                            bottom: values[3],
                        }
                        .normalized();
                    }
                }
                "straighten" => recipe.straighten = number().unwrap_or(0.0).clamp(-10.0, 10.0),
                "exposure" => recipe.exposure = number().unwrap_or(0.0).clamp(-2.0, 2.0),
                "fill" => recipe.fill_light = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "highlights" => recipe.highlights = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "shadows" => recipe.shadows = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "temp" => recipe.temperature = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "sat" => recipe.saturation = number().unwrap_or(0.0).clamp(-1.0, 1.0),
                "autocontrast" => recipe.auto_contrast = flag(),
                "autocolor" => recipe.auto_color = flag(),
                "bw" => recipe.black_white = flag(),
                "sepia" => recipe.sepia = flag(),
                "sharpen" => recipe.sharpen = number().unwrap_or(0.0).clamp(0.0, 1.0),
                _ => {}
            }
        }
        recipe
    }

    pub fn encode(&self) -> String {
        if self.is_default() {
            return String::new();
        }
        format!(
            "v=1|crop={:.5},{:.5},{:.5},{:.5}|straighten={:.4}|exposure={:.4}|fill={:.4}|highlights={:.4}|shadows={:.4}|temp={:.4}|sat={:.4}|autocontrast={}|autocolor={}|bw={}|sepia={}|sharpen={:.4}",
            self.crop.left,
            self.crop.top,
            self.crop.right,
            self.crop.bottom,
            self.straighten,
            self.exposure,
            self.fill_light,
            self.highlights,
            self.shadows,
            self.temperature,
            self.saturation,
            self.auto_contrast as u8,
            self.auto_color as u8,
            self.black_white as u8,
            self.sepia as u8,
            self.sharpen,
        )
    }

    pub fn is_default(&self) -> bool {
        self.crop.is_full()
            && self.straighten.abs() < 0.0001
            && self.exposure.abs() < 0.0001
            && self.fill_light.abs() < 0.0001
            && self.highlights.abs() < 0.0001
            && self.shadows.abs() < 0.0001
            && self.temperature.abs() < 0.0001
            && self.saturation.abs() < 0.0001
            && !self.auto_contrast
            && !self.auto_color
            && !self.black_white
            && !self.sepia
            && self.sharpen.abs() < 0.0001
    }
}

#[derive(Default)]
pub struct EditSession {
    pub recipe: EditRecipe,
    undo: Vec<EditRecipe>,
    redo: Vec<EditRecipe>,
    active_action: Option<EditRecipe>,
}

impl EditSession {
    pub fn new(recipe: EditRecipe) -> Self {
        Self {
            recipe,
            undo: Vec::new(),
            redo: Vec::new(),
            active_action: None,
        }
    }

    pub fn replace(&mut self, next: EditRecipe) {
        if self.recipe == next {
            return;
        }
        self.undo.push(self.recipe.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.recipe = next;
        self.redo.clear();
    }

    pub fn mutate(&mut self, update: impl FnOnce(&mut EditRecipe)) {
        let mut next = self.recipe.clone();
        update(&mut next);
        self.replace(next);
    }

    /// Begin one continuous UI action, such as dragging a slider. Intermediate
    /// values update the preview immediately, but only the recipe that existed
    /// before the drag is added to Undo when the action finishes.
    pub fn begin_action(&mut self) {
        if self.active_action.is_none() {
            self.active_action = Some(self.recipe.clone());
        }
    }

    pub fn mutate_active(&mut self, update: impl FnOnce(&mut EditRecipe)) {
        update(&mut self.recipe);
        self.redo.clear();
    }

    pub fn end_action(&mut self) {
        let Some(previous) = self.active_action.take() else {
            return;
        };
        if previous == self.recipe {
            return;
        }
        self.undo.push(previous);
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        self.end_action();
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.recipe.clone());
        self.recipe = previous;
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_action();
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.recipe.clone());
        self.recipe = next;
        true
    }

    pub fn reset(&mut self) {
        self.replace(EditRecipe::default());
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_round_trip() {
        let mut recipe = EditRecipe::default();
        recipe.crop = CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.8,
            bottom: 0.9,
        };
        recipe.exposure = 0.7;
        recipe.auto_color = true;
        recipe.sepia = true;
        recipe.sharpen = 0.4;
        let decoded = EditRecipe::decode(&recipe.encode());
        assert!((decoded.crop.left - 0.1).abs() < 0.001);
        assert!((decoded.exposure - 0.7).abs() < 0.001);
        assert!(decoded.auto_color);
        assert!(decoded.sepia);
        assert!((decoded.sharpen - 0.4).abs() < 0.001);
    }

    #[test]
    fn session_keeps_bw_and_sepia_independently() {
        let mut session = EditSession::new(EditRecipe::default());
        session.mutate(|recipe| recipe.black_white = true);
        session.mutate(|recipe| recipe.sepia = true);
        assert!(session.recipe.black_white);
        assert!(session.recipe.sepia);
        let decoded = EditRecipe::decode(&session.recipe.encode());
        assert!(decoded.black_white);
        assert!(decoded.sepia);
    }

    #[test]
    fn composed_crop_stays_normalized() {
        let outer = CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        let inner = CropRect {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let result = outer.compose(inner);
        assert!((result.left - 0.3).abs() < 0.001);
        assert!((result.right - 0.7).abs() < 0.001);
    }

    #[test]
    fn continuous_slider_drag_is_one_undo_action() {
        let mut session = EditSession::new(EditRecipe::default());
        session.begin_action();
        session.mutate_active(|recipe| recipe.exposure = 0.2);
        session.mutate_active(|recipe| recipe.exposure = 0.7);
        session.mutate_active(|recipe| recipe.exposure = 1.1);
        session.end_action();

        assert!((session.recipe.exposure - 1.1).abs() < 0.001);
        assert!(session.undo());
        assert!(session.recipe.exposure.abs() < 0.001);
        assert!(!session.undo());
        assert!(session.redo());
        assert!((session.recipe.exposure - 1.1).abs() < 0.001);
    }
}
