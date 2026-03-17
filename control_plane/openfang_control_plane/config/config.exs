import Config

dsn =
  case System.get_env("SENTRY_DSN") do
    nil -> nil
    "" -> nil
    other -> other
  end

config :openfang_control_plane,
  ecto_repos: [OpenfangControlPlane.Repo]

config :openfang_control_plane, OpenfangControlPlane.Repo,
  database: System.get_env("OFCP_DB_PATH", "./data/ofcp.sqlite3"),
  pool_size: 5,
  stacktrace: false,
  show_sensitive_data_on_connection_error: false

config :logger,
  level: String.to_atom(System.get_env("OFCP_LOG_LEVEL", "info"))

config :sentry,
  dsn: dsn,
  client: Sentry.HackneyClient,
  environment_name: System.get_env("SENTRY_ENV", "production"),
  enable_source_code_context: false,
  before_send: {OpenfangControlPlane.Sentry, :before_send},
  tags: %{
    "subsystem" => "control_plane"
  }
