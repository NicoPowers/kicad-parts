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
The PocketBase server one team shares. A person may work for several teams, one server each, and stays signed in to all of them at once. The catalog view shows one team at a time.
_Avoid_: backend, instance, workspace, account

**Sign-in**:
What lets a person use a team server without typing the password each time. The app keeps it in the computer's password store, never the password itself. Signing out forgets it.
_Avoid_: session, token, credentials, login

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
