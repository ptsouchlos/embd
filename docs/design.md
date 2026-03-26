# Design of `embd`

The basic goal of `embd` is to provide a simple way to "embed" one git repository into another. The prime source of inspiration for this project is the [beman-submodule](https://github.com/bemanproject/beman-submodule) tool, which is implemented in Python and is available on PyPI. `embd`, on the other hand, is implemented in Rust and will be available as a static binary which provides many options for distibution and installation.

An additional goal of `embd` is to be more general purpose than `beman-submodule` which is more closely tied to the goals and setup that the various `Beman` projects use.

## Why?

Both `beman-submodule` and `embd` aim to be alternatives to either [git submodules]() and [git subtree](). [Eddie Nolan](https://www.ednolan.com) put together a set of [slides](https://www.ednolan.com/toolchains_slides.pdf) to show the differences and limitations of both `git subtree` and `git submodule`, so I won't go into that too much here. The gist is that `git submodule` provides a subpar user experience (users have to remember to run `git submodule update --init`) and `git subtree`s force merge commits in your history.

## What's the solution?

The general idea is to pull the dependant repository as source into the parent repository. This puts the burden of keeping dependencies up to date on the maintainer. `embd` helps in making that process easier for the maintainer.

## Design

### Commands

| Command  |                 Description                 |                                                        Options                                                         |
| :------: | :-----------------------------------------: | :--------------------------------------------------------------------------------------------------------------------: |
|  `add`   |   Add a new embedded repo to the project.   | `path`: Specify what subdirectory to put embed in. \n `allow-untracked`: Allow untracked files in the embed directory. |
| `update` | Update all embeds to match the config file. |                                                                                                                        |
| `status` |       Show the status of all embeds.        |                                                                                                                        |

### Config File

The configuration file describes all the embedded sources.

```toml
[repo1]
remote="https://example.git"
commit_hash=123abcd1234
folder=/example

[repo2]
remote="https://example2.git"
commit_hash=123abcd1234
folder=/example2
```

### Cache

In addition to the config file, `embd` maintains a cache to check for differences in the local files and changes to the subfolder an embed is put into.
