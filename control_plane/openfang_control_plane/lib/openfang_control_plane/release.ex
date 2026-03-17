defmodule OpenfangControlPlane.Release do
  @moduledoc false

  def migrate do
    load_app()

    for repo <- Application.fetch_env!(:openfang_control_plane, :ecto_repos) do
      {:ok, _, _} =
        Ecto.Migrator.with_repo(repo, fn repo ->
          Ecto.Migrator.run(repo, :up, all: true)
        end)
    end
  end

  defp load_app do
    Application.load(:openfang_control_plane)
  end
end

