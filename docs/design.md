# Design of `embd`

The basic goal of `embd` is to provide a simple way to "embed" one git repository into another. The prime source of inspiration for this project is the [beman-submodule](https://github.com/bemanproject/beman-submodule) tool, which is implemented in Python and is available on PyPI. `embd`, on the other hand, is implemented in Rust and will be available as a static binary which provides many options for distibution and installation.

An additional goal of `embd` is to be more general purpose than `beman-submodule` which is more closely tied to the goals and setup that the various `Beman` projects use.

## Why?

Both `beman-submodule` and `embd` aim to be alternatives to either [git submodules](https://git-scm.com/book/en/v2/Git-Tools-Submodules) and [git subtree](https://www.atlassian.com/git/tutorials/git-subtree). [Eddie Nolan](https://www.ednolan.com) put together a set of [slides](https://www.ednolan.com/toolchains_slides.pdf) to show the differences and limitations of both `git subtree` and `git submodule`, so I won't go into that too much here. The gist is that `git submodule` provides a subpar user experience (users have to remember to run `git submodule update --init`) and `git subtree`s force merge commits in your history.

## What's the solution?

The general idea is to pull the dependant repository as source into the parent repository. This puts the burden of keeping dependencies up to date on the maintainer. `embd` helps in making that process easier for the maintainer.

## Design

### Commands

| Command  |                 Description                 |                                                                                                                                                 Options                                                                                                                                                 |
| :------: | :-----------------------------------------: | :-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------: |
|  `add`   |   Add a new embedded repo to the project.   | `folder`: Specify what subdirectory to put embed in. \n `allow-untracked`: Allow untracked files in the embed directory. \n `link`: The link to the project to include \n `rev`: Specify the commit,tag or branch to pull \n `include`: Glob patterns to include \n `exclude`: Glob patterns to exclude |
| `update` | Update all embeds to match the config file. |                                          `rev`: Specify the commit, tag or branch to advance to \n `force`: Overwrite any local modification \n `overwrite`: Delete all untracked files (requries `force`) \n `quiet`: Print summaries, not each file updates.                                          |
| `status` |       Show the status of all embeds.        |                                                                                                                           `quiet`: Should summaries per embed, not per file.                                                                                                                            |

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

### Lockfile

In addition to the config file, `embd` maintains a lockfile to check for differences in the local files and changes to the subfolder an embed is put into. This serves as an easy way to verify file status without pull the files again from the remote location.
