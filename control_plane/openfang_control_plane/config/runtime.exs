import Config

if config_env() == :prod do
  config :openfang_control_plane, OpenfangControlPlane.Repo,
    database: System.get_env("OFCP_DB_PATH", "/app/data/ofcp.sqlite3")

  dsn =
    case System.get_env("SENTRY_DSN") do
      nil -> nil
      "" -> nil
      other -> other
    end

  # Sentry must see `nil` when disabled; empty string crashes the app.
  config :sentry,
    dsn: dsn,
    client: Sentry.HackneyClient,
    environment_name: System.get_env("SENTRY_ENV", "production"),
    enable_source_code_context: false,
    tags: %{"subsystem" => "control_plane", "service" => "openfang_control_plane"}
end
