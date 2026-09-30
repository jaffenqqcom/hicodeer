use cloud_api_types::Plan;
use ui::{Chip, prelude::*};

/// A [`Chip`] that displays a [`Plan`].
#[derive(IntoElement)]
pub struct PlanChip {
    plan: Plan,
}

impl PlanChip {
    pub fn new(plan: Plan) -> Self {
        Self { plan }
    }
}

impl RenderOnce for PlanChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let free_chip_bg = cx
            .theme()
            .colors()
            .editor_background
            .opacity(0.5)
            .blend(cx.theme().colors().text_accent.opacity(0.05));

        let pro_chip_bg = cx
            .theme()
            .colors()
            .editor_background
            .opacity(0.5)
            .blend(cx.theme().colors().text_accent.opacity(0.2));

        let (plan_name, label_color, bg_color) = match self.plan {
            Plan::ZedFree => (localization::localized_str!("Free"), Color::Default, free_chip_bg),
            Plan::ZedProTrial => (localization::localized_str!("Pro Trial"), Color::Accent, pro_chip_bg),
            Plan::ZedPro => (localization::localized_str!("Pro"), Color::Accent, pro_chip_bg),
            Plan::ZedBusiness => (localization::localized_str!("Business"), Color::Accent, pro_chip_bg),
            Plan::ZedVip => (localization::localized_str!("VIP"), Color::Accent, pro_chip_bg),
            Plan::ZedStudent => (localization::localized_str!("Student"), Color::Accent, pro_chip_bg),
        };

        Chip::new(plan_name.to_string())
            .bg_color(bg_color)
            .label_color(label_color)
    }
}
