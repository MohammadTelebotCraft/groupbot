use grammers_client::message::Button;
use grammers_client::tl;

#[derive(Clone, Copy)]
pub enum Colour {
    Primary,

    Success,

    Danger,
}

impl Colour {
    fn style(self) -> tl::types::KeyboardButtonStyle {
        tl::types::KeyboardButtonStyle {
            bg_primary: matches!(self, Self::Primary),
            bg_danger: matches!(self, Self::Danger),
            bg_success: matches!(self, Self::Success),
            icon: None,
        }
    }
}

pub fn paint(button: Button, colour: Colour) -> Button {
    let style = Some(colour.style().into());
    let raw = match button.raw {
        tl::enums::KeyboardButton::Callback(mut b) => {
            b.style = style;
            tl::enums::KeyboardButton::Callback(b)
        }
        tl::enums::KeyboardButton::Url(mut b) => {
            b.style = style;
            tl::enums::KeyboardButton::Url(b)
        }
        other => other,
    };
    Button { raw }
}

pub fn data(text: impl Into<String>, payload: impl Into<Vec<u8>>, colour: Colour) -> Button {
    paint(Button::data(text, payload), colour)
}

pub fn toggle(text: impl Into<String>, payload: impl Into<Vec<u8>>, on: bool) -> Button {
    let button = Button::data(text, payload);
    if on { paint(button, Colour::Success) } else { button }
}

pub fn choice(text: impl Into<String>, payload: impl Into<Vec<u8>>, chosen: bool) -> Button {
    let button = Button::data(text, payload);
    if chosen {
        paint(button, Colour::Primary)
    } else {
        button
    }
}
