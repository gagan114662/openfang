defmodule OpenfangControlPlane.Application do
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      OpenfangControlPlane.Repo,
      {Task.Supervisor, name: OpenfangControlPlane.TaskSupervisor},
      OpenfangControlPlane.Telegram.AdminState,
      OpenfangControlPlane.Scheduler,
      OpenfangControlPlane.Telegram.Poller
    ]

    opts = [strategy: :one_for_one, name: OpenfangControlPlane.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
