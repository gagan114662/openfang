defmodule OpenfangControlPlane.Telegram.Client do
  @moduledoc false

  def token do
    System.get_env("TELEGRAM_BOT_TOKEN") || ""
  end

  def api_base do
    "https://api.telegram.org/bot" <> token()
  end

  def send_message(chat_id, text) do
    Req.post!(api_base() <> "/sendMessage",
      json: %{
        "chat_id" => chat_id,
        "text" => text,
        "parse_mode" => "Markdown"
      }
    ).body
  end

  def edit_message(chat_id, message_id, text) do
    Req.post!(api_base() <> "/editMessageText",
      json: %{
        "chat_id" => chat_id,
        "message_id" => message_id,
        "text" => text,
        "parse_mode" => "Markdown"
      }
    ).body
  end
end

