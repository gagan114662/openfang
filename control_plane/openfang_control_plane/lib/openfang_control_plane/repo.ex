defmodule OpenfangControlPlane.Repo do
  use Ecto.Repo,
    otp_app: :openfang_control_plane,
    adapter: Ecto.Adapters.SQLite3
end

