# Collaboration

The team, its members and what each one may change. Every other context saves its changes through Collaboration.

## Language

### People

**Member**:
A person in the team. Each member has one role and one priority.
_Avoid_: user, account, collaborator

**Role**:
What a member may do: Owner, Engineer, Purchaser or Viewer.
_Avoid_: Edit access, permission level, group

**Owner**:
The member who runs the team. The Owner gives members their roles and priorities, and can change anything.
_Avoid_: admin, superuser

**Engineer**:
A member who changes part details and CAD items.
_Avoid_: editor, designer

**Purchaser**:
A member who handles buying, such as supplier data and orders, but not part details.
_Avoid_: buyer, procurement user

**Viewer**:
A member who can see everything but change nothing.
_Avoid_: read-only user, guest

### The team

**Team server**:
The PocketBase server one team shares. A person may work for several teams, one server each, and stays signed in to all of them at once. One part list shows every team's parts, and each part is named by its team and its IPN, because two teams may use the same IPN.
_Avoid_: backend, instance, workspace, account

**Sign-in**:
What lets a person use a team server without typing the password each time. The app keeps it in the computer's password store, never the password itself. Signing out forgets it.
_Avoid_: session, token, credentials, login

**Last-known parts**:
A team's parts as the app last saw them from its team server. The app keeps them in the team's parts file, so a team whose server is down at start still shows them, marked offline, with when they were last up to date. Signing out or removing the team deletes the file.
_Avoid_: cache, offline copy, snapshot

**Parts file**:
The file on this computer that keeps one team's last-known parts between runs.
_Avoid_: kept parts, cache file

**Library repo**:
The git repo that holds a team's own KiCad library, often a client's ad hoc one, on any git host. The person links a team to it from their own clone or its address. The app reaches it with the person's own git and SSH setup, and keeps a copy of its own. Teams that link one repo share that copy. Where the repo keeps its libraries is its library layout (see Library Sources).
_Avoid_: client repo, remote, library source, git library

**Team colour**:
The colour of a team's mark in the part list and its name in the Teams menu. The app gives each team one when it joins the list, and keeps it. The mark always shows the team code too, never colour alone.
_Avoid_: team badge colour, label colour

**Team code**:
Two or three capital letters or digits, such as `MA`, that name a team's KiCad libraries: `MA-Parts`, `MA-Resistor_SMD`. It belongs to the team, not to a person, so every member gets the same names. Boards save those names, so the code is chosen once and never changes. Until roles exist, any member who can edit the Catalog chooses it.
_Avoid_: prefix, team ID, client code

### Changing things together

**Priority**:
A number the Owner gives each member. When two members change the same field, the higher priority wins. With equal priority, the app asks which value to keep.
_Avoid_: rank, seniority, precedence

**Change request**:
A change sent by someone without enough access or priority. It waits for a member with higher priority to accept or reject it.
_Avoid_: pull request, PR, suggestion, proposal
