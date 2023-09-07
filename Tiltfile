allow_k8s_contexts('orbstack')

docker_build(
	"ghcr.io/worldcoin/wallet-bridge", ".",
	dockerfile="Dockerfile",
)

k8s_yaml(helm('./deploy', 'world-id-bridge'))

k8s_resource(
  workload='world-id-bridge',
  port_forwards=8000
)
