# embd

`embd` is a CLI tool that serves as an alternative to `git subtree` and `git submodule`. It's heavily inspired by [beman-submodule](https://github.com/bemanproject/beman-submodule).

See the [design documentation](./docs/design.md) for more details on some of the differences between `embd` and `beman-submodule` as well as why those decisions we made.

## Installation

Since `embd` is still under heavy development while I work towards a first release, only local installs are currently supported. The easiest way to do this is with [just](https://github.com/casey/just) or `cargo` with one of the following commands:

```bash
# with just
just install
# or with cargo
cargo install --path .
```

## Usage

### Embed a Repository

```bash
embd add -l <repo link>.git -f <local folder>
```

This will clone the repository to the given folder and create a `config.toml` and `embd.lock` file in the `.embd` folder. These files should be commited to VCS. You can also filter the contents of the repository using `-i` and `-e` flags. These use glob filters to include or exclude certain folders or files. For example, to exclude all Markdown and include all text files, you could the following:

```bash
embd add -l <repo> -f <folder> -i "**.txt" -e "**.md"
```

### Update embeds

To update all embedded projects, run:

```bash
embd update
```

To update a specific project, use the name of the repo. This corresponds to the key of the projects entry in the `.embd/config.toml` file:

```bash
embd update infra
```

To update a project to a new commit, tag or branch, use the `-r` or `--rev` flag:

```bash
embd update infra --rev abcd1234
```

This will update the files on disk and update the commit hash tracked in the config file and lock file. In addition, the entries in the lock file for the given project will also be updated. Updates can also be forced using `--force` and untracked files can be removed using `--overwrite`. See `embd update -h` for more details.

### Check Status of Embeds

To check for deviations, run:

```bash
embd status
```

This will print out any files that differ from the tracked revision of the pulled files. This is mostly useful for ensuring that files that are part of an "embed" are not inadvertently edited. This check can be used in CI workflows to ensure that such edits do not occur.

## Credits

Credit to the [Beman Project](https://bemanproject.org/) and the [beman-submodule](https://github.com/bemanproject/beman-submodule) tool for the inspiration and initial idea for a tool like this.

## Author

| [<img src="https://avatars0.githubusercontent.com/u/6591180?s=460&v=4" width="100"><br><sub>@ptsouchlos</sub>](https://github.com/ptsouchlos) |
| :-------------------------------------------------------------------------------------------------------------------------------------------: |
