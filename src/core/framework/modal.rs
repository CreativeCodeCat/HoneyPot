//! Pop-up forms (modals) shown in reply to a command.

use std::time::Duration;

use ::serenity::all as serenity;

use super::Context;

/// A form: how to show it, and how to read what was typed in.
pub trait Modal: Sized {
    /// The form, with `defaults` filled in, answering with `custom_id`
    fn create(defaults: Option<Self>, custom_id: String) -> serenity::CreateInteractionResponse;
    fn parse(data: serenity::ModalInteractionData) -> Result<Self, &'static str>;
}

/// A text box in a form.
pub struct TextField<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub placeholder: &'a str,
    pub max_length: u16,
    /// Several lines rather than one
    pub paragraph: bool,
    pub required: bool,
    /// Filled in to start with
    pub value: Option<String>,
}

impl TextField<'_> {
    pub fn create(self) -> serenity::CreateActionRow {
        let style = if self.paragraph { serenity::InputTextStyle::Paragraph } else { serenity::InputTextStyle::Short };
        let mut input = serenity::CreateInputText::new(style, self.label, self.id)
            .placeholder(self.placeholder)
            .max_length(self.max_length)
            .required(self.required);
        if let Some(value) = self.value {
            input = input.value(value);
        }
        serenity::CreateActionRow::InputText(input)
    }
}

/// The form as the response to an interaction.
pub fn create_modal(custom_id: String, title: &str, fields: Vec<TextField<'_>>) -> serenity::CreateInteractionResponse {
    let rows = fields.into_iter().map(TextField::create).collect();
    serenity::CreateInteractionResponse::Modal(serenity::CreateModal::new(custom_id, title).components(rows))
}

/// The text typed into the field `custom_id`, or `None` if it was left blank.
pub fn find_modal_text(data: &mut serenity::ModalInteractionData, custom_id: &str) -> Option<String> {
    data.components
        .iter_mut()
        .filter_map(|row| match row.components.get_mut(0) {
            Some(serenity::ActionRowComponent::InputText(text)) => Some(text),
            _ => None,
        })
        .find(|text| text.custom_id == custom_id)
        .and_then(|text| std::mem::take(&mut text.value))
        .filter(|value| !value.is_empty())
}

/// Shows the form as the reply to the command and waits for it to be sent, for up to
/// `timeout` (an hour by default). `None` if it wasn't sent in time.
pub async fn execute_modal<M: Modal>(ctx: Context<'_>, defaults: Option<M>, timeout: Option<Duration>) -> Result<Option<M>, serenity::Error> {
    let custom_id = ctx.interaction.id.to_string();
    ctx.interaction.create_response(ctx, M::create(defaults, custom_id.clone())).await?;
    ctx.mark_responded();
    let response = serenity::ModalInteractionCollector::new(&ctx.serenity_context.shard)
        .filter(move |m| m.data.custom_id == custom_id)
        .timeout(timeout.unwrap_or(Duration::from_secs(3600)))
        .await;
    let Some(response) = response else { return Ok(None) };
    response.create_response(ctx, serenity::CreateInteractionResponse::Acknowledge).await?;
    Ok(Some(M::parse(response.data.clone()).map_err(serenity::Error::Other)?))
}
