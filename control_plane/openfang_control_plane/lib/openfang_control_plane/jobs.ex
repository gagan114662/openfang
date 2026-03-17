defmodule OpenfangControlPlane.Jobs do
  import Ecto.Query, warn: false
  alias OpenfangControlPlane.Repo
  alias OpenfangControlPlane.Jobs.Job

  def create(attrs) do
    %Job{}
    |> Job.changeset(attrs)
    |> Repo.insert()
  end

  def get(id), do: Repo.get(Job, id)

  def update(%Job{} = job, attrs) do
    job
    |> Job.changeset(attrs)
    |> Repo.update()
  end

  def latest_for_chat(chat_id, limit \\ 20) do
    Job
    |> where([j], j.telegram_chat_id == ^chat_id)
    |> order_by([j], desc: j.inserted_at)
    |> limit(^limit)
    |> Repo.all()
  end

  def count_running do
    Job
    |> where([j], j.status in ["queued", "running", "stopping"])
    |> select([j], count(j.id))
    |> Repo.one()
    |> case do
      nil -> 0
      n -> n
    end
  end
end
