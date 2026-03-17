defmodule OpenfangControlPlane.MixProject do
  use Mix.Project

  def project do
    [
      app: :openfang_control_plane,
      version: "0.1.0",
      elixir: "~> 1.17",
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      releases: releases()
    ]
  end

  def application do
    [
      # Include :hackney explicitly since Sentry transport uses it in production.
      extra_applications: [:logger, :runtime_tools, :hackney],
      mod: {OpenfangControlPlane.Application, []}
    ]
  end

  defp deps do
    [
      {:jason, "~> 1.4"},
      {:req, "~> 0.5"},
      {:websockex, "~> 0.4"},
      {:ecto_sql, "~> 3.12"},
      {:ecto_sqlite3, "~> 0.17"},
      {:uuid, "~> 1.1"},
      {:sentry, "~> 10.8"},
      # Sentry v10 defaults to Hackney unless configured otherwise.
      {:hackney, "~> 1.20"}
    ]
  end

  defp releases do
    [
      openfang_control_plane: [
        include_executables_for: [:unix],
        steps: [:assemble]
      ]
    ]
  end
end
