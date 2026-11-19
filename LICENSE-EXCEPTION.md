# Alaya System Library Exception

Alaya is licensed under the GNU General Public License, version 3 only (see
`LICENSE`). This file adds one permission on top of it, under section 7 of the
GPL. It exists so that apps and games written for Alaya may use any license,
while the kernel, servers and drivers stay GPL (design decision 20).

## Which code it covers

A **System Library** is a library in this repository whose source files carry
this identifier:

    SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception

and which is published as part of Alaya for applications to link against
(for example the SDK and its C interface, and the POSIX layer).

The kernel, the system servers, the drivers and any file carrying only
`SPDX-License-Identifier: GPL-3.0-only` are **not** System Libraries. This
exception does not apply to them.

At milestone 0 no System Library exists yet.

## The permission

As a special exception, the copyright holders of a System Library give you
permission to link or combine the System Library, unmodified or modified, with
an independent program (an "Application"), statically or dynamically, and to
convey the resulting combined work under terms of your choice, provided that:

1. the Application communicates with the System Library only through the
   library's published interface, and does not otherwise contain code taken
   from Alaya; and
2. you comply with the GPL for the System Library itself: if you convey a
   modified System Library, its source code remains available under the GPL
   with this exception.

An "independent program" is one that is not itself derived from the Alaya
kernel, servers or drivers.

## Modified versions

If you modify a System Library, you may extend this exception to your
modified version, but you are not obliged to. If you do not wish to, delete
this exception statement from your version.
