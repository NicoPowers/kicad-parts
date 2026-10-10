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

### Changing things together

**Priority**:
A number the Owner gives each member. When two members change the same field, the higher priority wins. With equal priority, the app asks which value to keep.
_Avoid_: rank, seniority, precedence

**Change request**:
A change sent by someone without enough access or priority. It waits for a member with higher priority to accept or reject it.
_Avoid_: pull request, PR, suggestion, proposal
