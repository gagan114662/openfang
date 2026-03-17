defmodule OpenfangControlPlane.Repo.Migrations.CreateJobs do
  use Ecto.Migration

  def change do
    create table(:jobs, primary_key: false) do
      add :id, :binary_id, primary_key: true

      add :telegram_chat_id, :text, null: false
      add :telegram_user_id, :text, null: false
      add :telegram_message_id, :integer

      add :agent_id, :text, null: false
      add :status, :text, null: false

      add :requested_provider, :text
      add :requested_model, :text

      add :ws_stream_key, :text
      add :ws_last_seq, :integer, null: false, default: 0

      add :started_at, :utc_datetime_usec
      add :finished_at, :utc_datetime_usec

      add :result_summary, :text
      add :error, :text

      timestamps(type: :utc_datetime_usec)
    end

    create index(:jobs, [:telegram_chat_id])
    create index(:jobs, [:agent_id])
    create index(:jobs, [:status])
  end
end

