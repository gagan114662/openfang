defmodule OpenfangControlPlane.OpenFang.HTTP do
  @moduledoc false

  def base_url do
    System.get_env("OPENFANG_HTTP_BASE", "http://127.0.0.1:50051")
  end

  def api_key do
    System.get_env("OPENFANG_API_KEY", "")
  end

  def headers(extra \\ []) do
    base =
      if api_key() != "" do
        [{"authorization", "Bearer " <> api_key()}]
      else
        []
      end

    base ++ extra
  end

  def list_agents do
    url = base_url() <> "/api/agents"
    Req.get!(url, headers: headers()) |> Map.fetch!(:body)
  end

  def stop_agent(agent_id) do
    url = base_url() <> "/api/agents/" <> agent_id <> "/stop"
    Req.post!(url, headers: headers([{"content-type", "application/json"}]), json: %{}) |> Map.fetch!(:body)
  end
end

