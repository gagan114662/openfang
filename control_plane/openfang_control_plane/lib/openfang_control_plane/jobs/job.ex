defmodule OpenfangControlPlane.Jobs.Job do
  use Ecto.Schema
  import Ecto.Changeset

  @primary_key {:id, :binary_id, autogenerate: true}
  @timestamps_opts [type: :utc_datetime_usec]

  schema "jobs" do
    field :telegram_chat_id, :string
    field :telegram_user_id, :string
    field :telegram_message_id, :integer

    field :agent_id, :string
    field :status, :string

    field :requested_provider, :string
    field :requested_model, :string

    field :ws_stream_key, :string
    field :ws_last_seq, :integer, default: 0

    field :started_at, :utc_datetime_usec
    field :finished_at, :utc_datetime_usec

    field :result_summary, :string
    field :error, :string

    timestamps()
  end

  def changeset(job, attrs) do
    job
    |> cast(attrs, [
      :telegram_chat_id,
      :telegram_user_id,
      :telegram_message_id,
      :agent_id,
      :status,
      :requested_provider,
      :requested_model,
      :ws_stream_key,
      :ws_last_seq,
      :started_at,
      :finished_at,
      :result_summary,
      :error
    ])
    |> validate_required([:telegram_chat_id, :telegram_user_id, :agent_id, :status])
  end
end

