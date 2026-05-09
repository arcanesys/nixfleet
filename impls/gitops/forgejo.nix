# Forgejo / Gitea raw-URL builder for channel-refs. Pure data, not a module.
{
  urlsFor = {
    baseUrl,
    owner,
    repo,
    ref ? "main",
    path ? "releases/fleet.resolved.json",
  }: let
    base = "${baseUrl}/${owner}/${repo}/raw/branch/${ref}/${path}";
  in {
    artifactUrl = base;
    signatureUrl = "${base}.sig";
  };
}
