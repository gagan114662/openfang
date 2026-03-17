defmodule OpenfangControlPlane.Sentry do
  @moduledoc false

  # Keep events small; enrich with stable tags.
  def before_send(event) do
    tags = Map.get(event, :tags, %{})
    event
    |> Map.put(:tags, Map.merge(tags, %{"service" => "openfang_control_plane"}))
  end
end

